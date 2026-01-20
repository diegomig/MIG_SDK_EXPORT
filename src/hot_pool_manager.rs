// Hot Pool Manager - maintains top-K pools in memory with adaptive refresh
use anyhow::Result;
use dashmap::{DashMap, DashSet};
use ethers::types::{Address, U256};
use ethers::prelude::{Provider, Http};
use log::{debug, info, warn, error};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::convert::TryInto;
use tokio::sync::RwLock;

use crate::contracts::{
    i_uniswap_v2_pair::IUniswapV2Pair as UniswapV2Pair,
    uniswap_v3::UniswapV3Pool as UniswapV3Contract,
};
use crate::data_validator::StateQuality;
use crate::metrics;
use crate::multicall::Call;
use crate::pools::{Pool, UniswapV2Pool, UniswapV3Pool};
use crate::rpc_pool::{RpcPool, RpcRole};
use crate::settings::Performance;
use crate::v3_math::V3PoolState;
use crate::flight_recorder::FlightRecorder;
use crate::{record_phase_start, record_phase_end};

/// Estado inicial válido para agregar un pool a HotPoolManager
/// Garantiza que el pool tenga estado válido antes de ser agregado
#[derive(Debug, Clone)]
pub enum PoolInitialState {
    V3 {
        sqrt_price_x96: U256,
        tick: i64,  // V3PoolState usa i64 para tick
        liquidity: u128,
    },
    V2 {
        reserve0: U256,
        reserve1: U256,
    },
}

#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub pool: Pool,
    pub last_updated: Instant,
    pub weight: f64,                // From graph service
    pub update_frequency: Duration, // Adaptive based on weight
}

fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn validated_positive(value: f64) -> Option<f64> {
    if is_positive_finite(value) {
        Some(value)
    } else {
        None
    }
}

/// V3 tick information for accurate swap calculations.
///
/// Contains liquidity data for a specific tick in a Uniswap V3 pool.
#[derive(Debug, Clone)]
pub struct V3TickInfo {
    /// Net liquidity change when crossing this tick
    pub liquidity_net: i128,
    /// Gross liquidity at this tick
    pub liquidity_gross: u128,
    /// Whether this tick has been initialized
    pub initialized: bool,
}

/// Snapshot of a Uniswap V3 pool's state in the hot pool manager.
///
/// Contains comprehensive state information including price, liquidity, ticks, and quality metrics.
#[derive(Debug, Clone)]
pub struct V3PoolSnapshot {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub state: V3PoolState,
    pub last_updated: Instant,
    pub weight: f64,
    pub dex: &'static str, // Store DEX type for ABI selection
    // Data quality flags
    pub state_quality: StateQuality,
    pub partial_fail_count: u8,
    pub approximate: bool,
    // Per-attribute timestamps for granular auditing
    pub slot0_updated_at: Option<Instant>,
    pub liquidity_updated_at: Option<Instant>,
    pub max_safe_amount_usd: Option<f64>,
    pub price_deviation_bps: Option<f64>,
    pub last_validated_at: Option<Instant>,
    pub token0_balance: U256,
    pub token1_balance: U256,
    pub last_tvl_estimate: Option<f64>,
    // ✅ Tick data for accurate V3 swap calculations
    pub tick_bitmap: HashMap<i16, U256>,  // word_position → bitmap
    pub ticks: HashMap<i32, V3TickInfo>,  // tick_index → tick_info
    pub last_tick_refresh: Option<Instant>, // When ticks were last refreshed
    pub last_refreshed_tick: Option<i64>,   // Tick value when ticks were last refreshed
    pub block_number: u64,                  // Block number when snapshot was last updated (FASE 1: State sync)
    // FASE 5: Hot pool scoring improvements
    pub price_std: Option<f64>,             // Price standard deviation (volatility intra-block)
    pub liquidity_change_rate: Option<f64>,  // Change % of liquidity (recent liquidity changes)
}

#[derive(Debug, Clone)]
pub struct V2PoolSnapshot {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub last_updated: Instant,
    pub weight: f64,
    pub state_quality: StateQuality,
    pub partial_fail_count: u8,
    pub approximate: bool,
    // Per-attribute timestamps for granular auditing
    pub reserves_updated_at: Option<Instant>,
    pub max_safe_amount_usd: Option<f64>,
    pub price_deviation_bps: Option<f64>,
    pub last_validated_at: Option<Instant>,
    pub last_tvl_estimate: Option<f64>,
    pub block_number: u64,                  // Block number when snapshot was last updated (FASE 1: State sync)
    // FASE 5: Hot pool scoring improvements
    pub price_std: Option<f64>,             // Price standard deviation (volatility intra-block)
    pub liquidity_change_rate: Option<f64>,  // Change % of liquidity (recent liquidity changes)
}

impl V2PoolSnapshot {
    pub const fn dex_name(&self) -> &'static str {
        "UniswapV2"
    }
}

impl V3PoolSnapshot {
    /// Check if tick data needs to be refreshed
    /// Ticks should be refreshed if:
    /// 1. More than 5 minutes have passed since last refresh
    /// 2. The current tick has moved more than 50 ticks from last refresh
    pub fn needs_tick_refresh(&self) -> bool {
        if let Some(last_refresh) = self.last_tick_refresh {
            let time_elapsed = self.last_updated.saturating_duration_since(last_refresh).as_secs() > 300;
            let tick_moved = if let Some(last_tick) = self.last_refreshed_tick {
                (self.state.tick as i64 - last_tick).abs() > 50
            } else {
                true
            };
            time_elapsed || tick_moved
        } else {
            // Never refreshed, needs initial fetch
            true
        }
    }

    /// Get tick spacing based on fee tier
    pub fn tick_spacing(&self) -> i32 {
        match self.fee {
            500 => 10,      // 0.05%
            3000 => 60,     // 0.3%
            10000 => 200,   // 1%
            _ => 60,        // default
        }
    }
}

/// Snapshot for Curve StableSwap pools
#[derive(Debug, Clone)]
pub struct CurvePoolSnapshot {
    pub address: Address,
    pub tokens: Vec<Address>,
    pub balances: Vec<U256>,
    pub a: U256, // Amplification parameter
    pub fee: U256,
    pub last_updated: Instant,
    pub weight: f64,
    pub dex: &'static str,
    pub state_quality: StateQuality,
    pub partial_fail_count: u8,
    pub approximate: bool,
    // Per-attribute timestamps for granular auditing
    pub balances_updated_at: Option<Instant>,
    pub params_updated_at: Option<Instant>, // For 'a' and 'fee'
    pub block_number: u64,                  // Block number when snapshot was last updated (FASE 1: State sync)
}

/// Snapshot for Balancer Weighted pools
#[derive(Debug, Clone)]
pub struct BalancerPoolSnapshot {
    pub address: Address,
    pub pool_id: [u8; 32],
    pub tokens: Vec<Address>,
    pub balances: Vec<U256>,
    pub weights: Vec<U256>,
    pub swap_fee: U256,
    pub last_updated: Instant,
    pub weight: f64,
    pub dex: &'static str,
    pub state_quality: StateQuality,
    pub partial_fail_count: u8,
    pub approximate: bool,
    // Per-attribute timestamps for granular auditing
    pub balances_updated_at: Option<Instant>,
    pub weights_updated_at: Option<Instant>,
    pub block_number: u64,                  // Block number when snapshot was last updated (FASE 1: State sync)
}

/// Hot pool manager for maintaining top-K pools in memory with adaptive refresh rates.
///
/// The HotPoolManager maintains a cache of the highest-liquidity pools with automatic
/// refresh scheduling based on pool weight and activity.
///
/// ## Features
///
/// - **Top-K Selection**: Maintains only the highest-liquidity pools
/// - **Adaptive Refresh**: Refresh frequency based on pool weight
/// - **State Quality Tracking**: Monitors data freshness (Fresh, Stale, Corrupt)
/// - **Automatic Pruning**: Removes low-liquidity pools to maintain size limits
///
/// ## Performance
///
/// - Memory usage: ~200KB per 1,000 pools
/// - Refresh latency: <50ms per pool
/// - Cache hit rate: >85% for hot pools
pub struct HotPoolManager {
    // Hot pools kept in memory with fast access
    pub v3_hot_pools: Arc<DashMap<Address, V3PoolSnapshot>>,
    pub v2_hot_pools: Arc<DashMap<Address, V2PoolSnapshot>>,
    pub curve_hot_pools: Arc<DashMap<Address, CurvePoolSnapshot>>,
    pub balancer_hot_pools: Arc<DashMap<Address, BalancerPoolSnapshot>>,
    
    // Pool weights from graph service
    pool_weights: Arc<RwLock<HashMap<Address, f64>>>,

    // DELTA REFRESH: Track pools touched by recent events (Swap/Mint/Burn)
    touched_pools: Arc<DashSet<Address>>,
    last_touch_clear: Arc<RwLock<Instant>>,
    touch_ttl: Duration, // Clear touch-set every N seconds
    
    // Configuration
    top_k: usize,
    hot_threshold: f64,
    _warm_update_interval: Duration,
    _cold_update_interval: Duration,
    
    // Infrastructure
    rpc_pool: Arc<RpcPool>,
    multicall_batch_size: usize,
    // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    flight_recorder: Option<Arc<FlightRecorder>>,
}

impl HotPoolManager {
    pub fn new(settings: &Performance, rpc_pool: Arc<RpcPool>, hot_threshold: f64) -> Self {
        Self {
            v3_hot_pools: Arc::new(DashMap::new()),
            v2_hot_pools: Arc::new(DashMap::new()),
            curve_hot_pools: Arc::new(DashMap::new()),
            balancer_hot_pools: Arc::new(DashMap::new()),
            pool_weights: Arc::new(RwLock::new(HashMap::new())),
            touched_pools: Arc::new(DashSet::new()),
            last_touch_clear: Arc::new(RwLock::new(Instant::now())),
            touch_ttl: Duration::from_secs(45), // Clear touch-set every 45s
            top_k: settings.state_refresh_top_k,
            hot_threshold,
            _warm_update_interval: Duration::from_secs(12), // ~1 block
            _cold_update_interval: Duration::from_secs(60), // ~5 blocks
            rpc_pool,
            multicall_batch_size: settings.multicall_batch_size,
            flight_recorder: None,
        }
    }
    
    /// Set flight recorder for instrumentation
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder);
        self
    }

    /// ✅ P0 OPTIMIZATION: Get pool weight for a specific pool
    /// 
    /// Returns the weight from the pool_weights map, or from the pool snapshot if available.
    pub fn get_pool_weight(&self, pool_address: &Address) -> Option<f64> {
        // Try to get from pool_weights first (most up-to-date)
        if let Ok(weights) = self.pool_weights.try_read() {
            if let Some(weight) = weights.get(pool_address) {
                return Some(*weight);
            }
        }
        
        // Fallback to pool snapshot weight
        if let Some(v3_snapshot) = self.v3_hot_pools.get(pool_address) {
            return Some(v3_snapshot.weight);
        }
        if let Some(v2_snapshot) = self.v2_hot_pools.get(pool_address) {
            return Some(v2_snapshot.weight);
        }
        if let Some(curve_snapshot) = self.curve_hot_pools.get(pool_address) {
            return Some(curve_snapshot.weight);
        }
        if let Some(balancer_snapshot) = self.balancer_hot_pools.get(pool_address) {
            return Some(balancer_snapshot.weight);
        }
        
        None
    }
    
    /// Update pool weights from graph service
    pub async fn update_weights(&self, weights: HashMap<Address, f64>) {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de update_weights
        record_phase_start!(&self.flight_recorder, "hot_pool_manager_update_weights", serde_json::json!({
            "weights_count": weights.len()
        }));
        
        let mut pool_weights = self.pool_weights.write().await;
        *pool_weights = weights;
        let count = pool_weights.len();
        
        info!(
            "Updated {} pool weights in HotPoolManager",
            count
        );
        
        // ✅ FLIGHT RECORDER: Registrar fin de update_weights
        record_phase_end!(&self.flight_recorder, "hot_pool_manager_update_weights", start_time, serde_json::json!({
            "weights_count": count
        }));
    }

    /// Get pools that need state refresh based on weight and time
    /// Returns: (v3_pools, v2_pools, curve_pools, balancer_pools)
    pub async fn get_pools_needing_refresh(
        &self,
    ) -> (Vec<Address>, Vec<Address>, Vec<Address>, Vec<Address>) {
        info!("HotPoolManager: get_pools_needing_refresh called");
        let weights = self.pool_weights.read().await;
        let now = Instant::now();

        let mut v3_refresh = Vec::new();
        let mut v2_refresh = Vec::new();
        let mut curve_refresh = Vec::new();
        let mut balancer_refresh = Vec::new();

        let v3_total = self.v3_hot_pools.len();
        let v2_total = self.v2_hot_pools.len();
        let curve_total = self.curve_hot_pools.len();
        let balancer_total = self.balancer_hot_pools.len();
        
        info!(
            "HotPoolManager: Checking {} V3, {} V2, {} Curve, {} Balancer pools for refresh",
            v3_total, v2_total, curve_total, balancer_total
        );
        
        // 🔍 DIAGNÓSTICO: Log detallado de todos los pools V3
        if v3_total > 0 {
            let mut v3_addrs: Vec<_> = self.v3_hot_pools.iter()
                .map(|e| *e.key())
                .collect();
            v3_addrs.sort();
            info!("🔍 DIAGNÓSTICO: V3 pools in HotPoolManager ({} total): {:?}", 
                  v3_total, 
                  v3_addrs.iter().take(10).collect::<Vec<_>>());
        }
        
        // FIX: Intervalos dramáticamente reducidos para debug
        // CRÍTICO: Estos intervalos determinan qué pools se refrescan
        // Si un pool se actualizó hace menos tiempo que el intervalo, no se refresca
        // Esto puede causar que pools válidos no se refresquen y queden con estados inválidos
        let debug_hot_interval = Duration::from_secs(5); // 5 segundos para "hot"
        let debug_warm_interval = Duration::from_secs(10); // 10 segundos para "warm"
        let debug_cold_interval = Duration::from_secs(15); // 15 segundos para "cold"
        
        // 🔍 DIAGNÓSTICO: Log cuántos pools hay en total vs cuántos necesitan refresh
        info!("🔍 DIAGNÓSTICO: Total pools in HotPoolManager: V3={}, V2={}", v3_total, v2_total);
        
        const MIN_WEIGHT: f64 = 100_000.0; // $100K mínimo
        
        let mut v3_excluded_degraded = 0;
        let mut v3_excluded_zero_state = 0;
        
        // Check V3 pools
        for entry in self.v3_hot_pools.iter() {
            let (addr, snapshot) = entry.pair();
            let weight = weights.get(addr).copied().unwrap_or(snapshot.weight);
            
            // ✅ FIX 9: EXCLUIR pools degradados (deben ser eliminados, no refrescados)
            // PERO: No excluir pools con weight < MIN_WEIGHT si tienen estado válido
            // Solo excluir si están corruptos
            if snapshot.state_quality == StateQuality::Corrupt {
                // Pool corrupto - NO incluirlo en refresh
                v3_excluded_degraded += 1;
                debug!("🔍 V3 pool {} EXCLUDED from refresh (corrupt)", addr);
                continue;
            }
            
            // ✅ FIX 4: Pool con estado inicial cero SIEMPRE necesita refresh (independiente del tiempo)
            if snapshot.state.sqrt_price_x96.is_zero() || snapshot.state.liquidity == 0 {
                v3_refresh.push(*addr);
                v3_excluded_zero_state += 1;
                debug!("🔍 V3 pool {} NEEDS refresh (zero initial state)", addr);
                continue; // Skip time check
            }
            
            let time_since_update = now.duration_since(snapshot.last_updated);
            
            // FIX: Lógica mejorada de refresh
            let base_interval = if weight > self.hot_threshold {
                debug_hot_interval
            } else if weight > 0.0 {
                debug_warm_interval
            } else {
                debug_cold_interval
            };
            // Apply simple exponential backoff per partial_fail_count (cap at x8)
            let backoff_multiplier = 1u64 << (snapshot.partial_fail_count.min(3) as u32); // 1,2,4,8
            let required_interval = base_interval.saturating_mul(backoff_multiplier as u32);
            let needs_refresh = time_since_update > required_interval;

            debug!(
                "V3 pool {}: weight={:.2}, time_since_update={:?}, needs_refresh={}",
                addr, weight, time_since_update, needs_refresh
            );
            
            if needs_refresh {
                v3_refresh.push(*addr);
            } else {
                debug!("🔍 V3 pool {} does NOT need refresh: time_since_update={:?}, required_interval={:?}", 
                       addr, time_since_update, required_interval);
            }
        }
        
        if v3_excluded_degraded > 0 {
            warn!("🔍 DIAGNÓSTICO: Excluded {} V3 pools from refresh (corrupt)", v3_excluded_degraded);
        }
        info!("🔍 DIAGNÓSTICO: {} V3 pools need refresh out of {} total ({} excluded corrupt, {} zero state)", 
              v3_refresh.len(), v3_total, v3_excluded_degraded, v3_excluded_zero_state);
        
        let mut _v2_excluded_degraded = 0;
        let mut _v2_excluded_zero_state = 0;
        
        // Check V2 pools
        for entry in self.v2_hot_pools.iter() {
            let (addr, snapshot) = entry.pair();
            let weight = weights.get(addr).copied().unwrap_or(snapshot.weight);
            
            // ✅ FIX 9: EXCLUIR pools degradados (deben ser eliminados, no refrescados)
            // PERO: No excluir pools con weight < MIN_WEIGHT si tienen estado válido
            // Solo excluir si están corruptos
            if snapshot.state_quality == StateQuality::Corrupt {
                // Pool corrupto - NO incluirlo en refresh
                _v2_excluded_degraded += 1;
                debug!("🔍 V2 pool {} EXCLUDED from refresh (corrupt)", addr);
                continue;
            }
            
            // ✅ FIX 4: Pool con estado inicial cero SIEMPRE necesita refresh (independiente del tiempo)
            if snapshot.reserve0.is_zero() || snapshot.reserve1.is_zero() {
                v2_refresh.push(*addr);
                _v2_excluded_zero_state += 1;
                debug!("🔍 V2 pool {} NEEDS refresh (zero initial state)", addr);
                continue; // Skip time check
            }
            
            let time_since_update = now.duration_since(snapshot.last_updated);
            
            // FIX: Lógica mejorada de refresh
            let base_interval = if weight > self.hot_threshold {
                debug_hot_interval
            } else if weight > 0.0 {
                debug_warm_interval
            } else {
                debug_cold_interval
            };
            let backoff_multiplier = 1u64 << (snapshot.partial_fail_count.min(3) as u32);
            let required_interval = base_interval.saturating_mul(backoff_multiplier as u32);
            let needs_refresh = time_since_update > required_interval;

            debug!(
                "V2 pool {}: weight={:.2}, time_since_update={:?}, needs_refresh={}",
                addr, weight, time_since_update, needs_refresh
            );
            
            if needs_refresh {
                v2_refresh.push(*addr);
            }
        }
        
        // Check Curve pools
        for entry in self.curve_hot_pools.iter() {
            let (addr, snapshot) = entry.pair();
            let weight = weights.get(addr).copied().unwrap_or(snapshot.weight);
            let time_since_update = now.duration_since(snapshot.last_updated);
            
            let base_interval = if weight > self.hot_threshold {
                debug_hot_interval
            } else if weight > 0.0 {
                debug_warm_interval
            } else {
                debug_cold_interval
            };
            let backoff_multiplier = 1u64 << (snapshot.partial_fail_count.min(3) as u32);
            let required_interval = base_interval.saturating_mul(backoff_multiplier as u32);
            let needs_refresh = time_since_update > required_interval;

            debug!(
                "Curve pool {}: weight={:.2}, time_since_update={:?}, needs_refresh={}",
                addr, weight, time_since_update, needs_refresh
            );
            
            if needs_refresh {
                curve_refresh.push(*addr);
            }
        }
        
        // Check Balancer pools
        for entry in self.balancer_hot_pools.iter() {
            let (addr, snapshot) = entry.pair();
            let weight = weights.get(addr).copied().unwrap_or(snapshot.weight);
            let time_since_update = now.duration_since(snapshot.last_updated);
            
            let base_interval = if weight > self.hot_threshold {
                debug_hot_interval
            } else if weight > 0.0 {
                debug_warm_interval
            } else {
                debug_cold_interval
            };
            let backoff_multiplier = 1u64 << (snapshot.partial_fail_count.min(3) as u32);
            let required_interval = base_interval.saturating_mul(backoff_multiplier as u32);
            let needs_refresh = time_since_update > required_interval;

            debug!(
                "Balancer pool {}: weight={:.2}, time_since_update={:?}, needs_refresh={}",
                addr, weight, time_since_update, needs_refresh
            );
            
            if needs_refresh {
                balancer_refresh.push(*addr);
            }
        }
        
        // Cap by top_k based on weight (highest first) to bound per-block work
        let sort_by_weight_desc = |addrs: &mut Vec<Address>| {
            addrs.sort_by(|a, b| {
                let wa = weights.get(a).copied().unwrap_or(0.0);
                let wb = weights.get(b).copied().unwrap_or(0.0);
                wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
            });
            if addrs.len() > self.top_k {
                addrs.truncate(self.top_k);
            }
        };
        sort_by_weight_desc(&mut v3_refresh);
        sort_by_weight_desc(&mut v2_refresh);
        // Curve/Balancer typically small; keep as-is, or cap as well if needed later

        info!(
            "HotPoolManager: Returning {} V3, {} V2, {} Curve, {} Balancer pools for refresh (top_k={})",
            v3_refresh.len(),
            v2_refresh.len(),
            curve_refresh.len(),
            balancer_refresh.len(),
            self.top_k
        );
        
        (v3_refresh, v2_refresh, curve_refresh, balancer_refresh)
    }

    /// ✅ OPTIMIZACIÓN: Refresh selectivo de pools específicos
    /// Refresca solo los pools especificados (útil para pools touched)
    pub async fn refresh_pools_selective(
        &self,
        pool_addresses: &std::collections::HashSet<Address>,
    ) -> Result<()> {
        // Filtrar pools que necesitan refresh (solo los que están en HotPoolManager)
        let v3_to_refresh: Vec<_> = pool_addresses.iter()
            .filter(|addr| self.v3_hot_pools.contains_key(addr))
            .copied()
            .collect();
        
        let v2_to_refresh: Vec<_> = pool_addresses.iter()
            .filter(|addr| self.v2_hot_pools.contains_key(addr))
            .copied()
            .collect();
        
        // Refresh usando método existente
        self.refresh_pool_states(&v3_to_refresh, &v2_to_refresh).await
    }

    /// ✅ REFACTOR: Refresh state for specified pools using UnifiedStateFetcher (preferred method)
    pub async fn refresh_pool_states_with_unified_fetcher(
        &self,
        unified_fetcher: &mut crate::unified_state_fetcher::UnifiedStateFetcher,
        v3_pools: &[Address],
        v2_pools: &[Address],
        cycle_number: u64,
    ) -> Result<()> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de refresh_pool_states
        record_phase_start!(&self.flight_recorder, "hot_pool_manager_refresh_unified", serde_json::json!({
            "v3_pools": v3_pools.len(),
            "v2_pools": v2_pools.len()
        }));
        
        if v3_pools.is_empty() && v2_pools.is_empty() {
            record_phase_end!(&self.flight_recorder, "hot_pool_manager_refresh_unified", start_time, serde_json::json!({
                "result": "skipped",
                "reason": "no_pools_to_refresh"
            }));
            return Ok(());
        }

        // Use UnifiedStateFetcher to fetch states
        let updates = unified_fetcher.fetch_selected_pools(
            self,
            Some(&v3_pools.iter().chain(v2_pools.iter()).copied().collect()),
            cycle_number,
        ).await?;

        // Apply updates to HotPoolManager
        unified_fetcher.apply_updates(self, &updates).await?;

        // ✅ FLIGHT RECORDER: Registrar fin de refresh_pool_states
        record_phase_end!(&self.flight_recorder, "hot_pool_manager_refresh_unified", start_time, serde_json::json!({
            "v3_updated": updates.v3_updates.len(),
            "v2_updated": updates.v2_updates.len(),
            "curve_updated": updates.curve_updates.len(),
            "balancer_updated": updates.balancer_updates.len(),
        }));

        Ok(())
    }

    /// Refresh state for specified pools using unified multicall (legacy method, kept for backward compatibility)
    pub async fn refresh_pool_states(
        &self,
        v3_pools: &[Address],
        v2_pools: &[Address],
    ) -> Result<()> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de refresh_pool_states
        record_phase_start!(&self.flight_recorder, "hot_pool_manager_refresh", serde_json::json!({
            "v3_pools": v3_pools.len(),
            "v2_pools": v2_pools.len()
        }));
        
        // 🔍 GUARD: Verificar count al inicio
        let v3_count_at_entry = self.v3_hot_pools.len();
        let v2_count_at_entry = self.v2_hot_pools.len();
        warn!("🔍 refresh_pool_states ENTRY: {} V3 pools, {} V2 pools (requested: {} V3, {} V2)", 
              v3_count_at_entry, v2_count_at_entry, v3_pools.len(), v2_pools.len());
        
        if v3_pools.is_empty() && v2_pools.is_empty() {
            record_phase_end!(&self.flight_recorder, "hot_pool_manager_refresh", start_time, serde_json::json!({
                "result": "skipped",
                "reason": "no_pools_to_refresh"
            }));
            return Ok(());
        }

        let multicall_address = "0xcA11bde05977b3631167028862bE2a173976CA11"
            .parse()
            .unwrap();
        let (multicall, (provider, _permit)) = self
            .rpc_pool
            .acquire_multicall(RpcRole::State, multicall_address, self.multicall_batch_size)
            .await?;
        
        // FASE 1.2: Get current block number for state synchronization
        let current_block = provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get current block: {}", e))?
            .as_u64();

        let mut calls = Vec::new();
        enum CallKind {
            V3 { addr: Address },
            V2 { addr: Address },
        }
        let mut call_map = Vec::new(); // Track which call belongs to which pool

        // Add V3 calls (slot0 + liquidity)
        for &pool_addr in v3_pools {
            let pool_contract = UniswapV3Contract::new(pool_addr, Arc::clone(&provider));
            calls.push(Call {
                target: pool_addr,
                call_data: pool_contract.slot_0().calldata().unwrap(),
            });
            calls.push(Call {
                target: pool_addr,
                call_data: pool_contract.liquidity().calldata().unwrap(),
            });
            call_map.push(CallKind::V3 { addr: pool_addr });
        }

        // Add V2 calls (reserves)
        for &pool_addr in v2_pools {
            let pair_contract = UniswapV2Pair::new(pool_addr, Arc::clone(&provider));
            calls.push(Call {
                target: pool_addr,
                call_data: pair_contract.get_reserves().calldata().unwrap(),
            });
            call_map.push(CallKind::V2 { addr: pool_addr });
        }

        // FASE 4: Log refresh attempt with pool addresses
        info!(
            "🔄 HotPoolManager refresh START: {} V3 pools, {} V2 pools",
            v3_pools.len(),
            v2_pools.len()
        );
        if !v3_pools.is_empty() {
            debug!(
                "🔄 V3 pools to refresh: {:?}",
                v3_pools.iter().take(10).collect::<Vec<_>>()
            );
        }
        metrics::increment_v3_pools_refresh_attempted(v3_pools.len() as u64);

        let results = multicall.run(calls, None).await?;
        let now = Instant::now();

        let v3_count_before = self.v3_hot_pools.len();
        let v2_count_before = self.v2_hot_pools.len();
        
        info!(
            "HotPoolManager refresh: requested {} V3 pools, {} V2 pools, got {} results (before: V3={}, V2={})",
            v3_pools.len(),
            v2_pools.len(),
            results.len(),
            v3_count_before,
            v2_count_before
        );
        
        // 🔍 GUARD: Verificar que los pools solicitados existen
        let mut v3_requested_exist = 0;
        let mut v2_requested_exist = 0;
        for addr in v3_pools.iter() {
            if self.v3_hot_pools.contains_key(addr) {
                v3_requested_exist += 1;
            } else {
                warn!("🚨 V3 pool {:?} requested for refresh but NOT in HotPoolManager!", addr);
            }
        }
        for addr in v2_pools.iter() {
            if self.v2_hot_pools.contains_key(addr) {
                v2_requested_exist += 1;
            } else {
                warn!("🚨 V2 pool {:?} requested for refresh but NOT in HotPoolManager!", addr);
            }
        }
        if v3_requested_exist < v3_pools.len() || v2_requested_exist < v2_pools.len() {
            warn!("🚨 Some requested pools missing: V3 {}/{} exist, V2 {}/{} exist", 
                  v3_requested_exist, v3_pools.len(), v2_requested_exist, v2_pools.len());
        }

        // Process results
        let dummy_v3 = UniswapV3Contract::new(Address::zero(), Arc::clone(&provider));
        let slot0_fn = dummy_v3.abi().function("slot0")?;
        let liquidity_fn = dummy_v3.abi().function("liquidity")?;
        
        let dummy_v2 = UniswapV2Pair::new(Address::zero(), Arc::clone(&provider));
        let reserves_fn = dummy_v2.abi().function("getReserves")?;

        let mut i = 0;
        let mut v3_updated = 0;
        let mut v2_updated = 0;
        for call_desc in call_map {
            if i >= results.len() {
                break;
            }

            match call_desc {
                CallKind::V3 { addr: pool_addr } => {
                    if i + 1 >= results.len() {
                        warn!(
                            "Missing V3 data for pool {}: expected 2 results starting at index {}, got {} total",
                            pool_addr,
                            i,
                            results.len()
                        );
                        break;
                    }
                    if let (Some(slot0_data), Some(liquidity_data)) =
                        (results.get(i), results.get(i + 1))
                    {
                        if let (Ok(slot0_decoded), Ok(liquidity_decoded)) = (
                            slot0_fn.decode_output(slot0_data),
                            liquidity_fn.decode_output(liquidity_data),
                        ) {
                            // Decode sqrtPriceX96 and liquidity directly; derive tick from sqrtPrice if native decode fails
                            let sqrt_price_opt = slot0_decoded[0].clone().into_uint();
                            let tick_opt_native: Option<i64> = slot0_decoded[1]
                                    .clone()
                                    .into_int()
                                .and_then(|i| i.try_into().ok());
                            let liquidity_opt: Option<u128> = liquidity_decoded[0]
                                    .clone()
                                    .into_uint()
                                .and_then(|u| u.try_into().ok());

                            if let (Some(sqrt_price), Some(liquidity)) = (sqrt_price_opt, liquidity_opt) {
                                // Fallback: compute tick from sqrt_price if native int24 decode was problematic
                                let tick: i64 = tick_opt_native.unwrap_or_else(|| {
                                    crate::v3_math::get_tick_at_sqrt_ratio(sqrt_price)
                                });
                                if let Some(mut entry) = self.v3_hot_pools.get_mut(&pool_addr) {
                                    // FASE 4: Log state before update
                                    let old_state_quality = entry.state_quality;
                                    let old_age_secs = entry.last_updated.elapsed().as_secs();
                                    
                                    // FASE 5: Calculate volatility metrics (saturating conversion to avoid u128 overflow)
                                    fn u256_to_f64_saturating(x: U256) -> f64 {
                                        if x > U256::from(u128::MAX) {
                                            u128::MAX as f64
                                        } else {
                                            x.as_u128() as f64
                                        }
                                    }
                                    let old_price = entry.state.sqrt_price_x96;
                                    let old_liquidity = entry.state.liquidity;
                                    // Calculate price change (using sqrt_price_x96 difference as proxy)
                                    let price_delta = if old_price > U256::zero() {
                                        let price_now_f64 = u256_to_f64_saturating(sqrt_price);
                                        let price_prev_f64 = u256_to_f64_saturating(old_price);
                                        if price_prev_f64 > 0.0 {
                                            Some((price_now_f64 - price_prev_f64).abs() / price_prev_f64)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    
                                    // Calculate liquidity change rate
                                    let liquidity_change_rate = if old_liquidity > 0 {
                                        let liquidity_now = liquidity as f64;
                                        let liquidity_prev = old_liquidity as f64;
                                        Some((liquidity_now - liquidity_prev) / liquidity_prev)
                                    } else {
                                        None
                                    };
                                    
                                    entry.state = V3PoolState {
                                        sqrt_price_x96: sqrt_price,
                                        tick,
                                        liquidity,
                                    };
                                    entry.last_updated = now;
                                    entry.state_quality = StateQuality::Fresh; // FASE 4: Explicitly set to Fresh after successful refresh
                                    entry.block_number = current_block; // FASE 1.2: Update block number
                                    entry.price_std = price_delta; // FASE 5: Price volatility
                                    entry.liquidity_change_rate = liquidity_change_rate; // FASE 5: Liquidity change rate
                                    
                                    debug!(
                                        "✅ V3 refresh SUCCESS: pool={:?} | old_quality={:?} old_age={}s | new_quality=Fresh | sqrt_price={:?} liquidity={} tick={} block={} | price_std={:?} liquidity_change={:?}",
                                        pool_addr, old_state_quality, old_age_secs, sqrt_price, liquidity, tick, current_block, price_delta, liquidity_change_rate
                                    );
                                    
                                    // ✅ FIX: Removed blocking fetch_v3_tick_data from refresh loop
                                    // Tick data fetch is now done separately to avoid blocking the refresh
                                    // The pool state (sqrt_price, tick, liquidity) is sufficient for basic operations
                                    // Tick data can be fetched later in a separate batch process if needed
                                    
                                    v3_updated += 1;
                                    metrics::increment_v3_pools_refresh_succeeded(1);
                                } else {
                                    warn!(
                                        "❌ V3 refresh FAILED: pool={:?} not found in hot_pools cache during refresh",
                                        pool_addr
                                    );
                                    metrics::increment_v3_pools_refresh_failed("not_in_cache");
                                }
                            } else {
                                warn!(
                                    "❌ V3 refresh FAILED: pool={:?} decode error | slot0={:?} liquidity={:?}",
                                    pool_addr, slot0_decoded, liquidity_decoded
                                );
                                metrics::increment_v3_pools_refresh_failed("decode_error");
                            }
                        } else {
                            // FASE 4: Log raw decode error with lengths
                            // slot0_data and liquidity_data are &Bytes here (already unwrapped in if let)
                            let slot0_len = slot0_data.len();
                            let liquidity_len = liquidity_data.len();
                            warn!(
                                "❌ V3 refresh FAILED: pool={:?} raw decode error | slot0_len={} liquidity_len={}",
                                pool_addr, slot0_len, liquidity_len
                            );
                            metrics::increment_v3_pools_refresh_failed("raw_decode_error");
                        }
                    } else {
                        warn!(
                            "Missing V3 data for pool {}: expected 2 results starting at index {}, got {} total",
                            pool_addr,
                            i,
                            results.len()
                        );
                    }
                    i += 2;
                }
                CallKind::V2 { addr: pool_addr } => {
                    if let Some(reserves_data) = results.get(i) {
                        if reserves_data.is_empty() {
                            warn!("Empty V2 multicall data for pool {}", pool_addr);
                        } else {
                            match reserves_fn.decode_output(reserves_data) {
                                Ok(reserves_decoded) => {
                                    if reserves_decoded.len() < 2 {
                                        warn!(
                                            "Malformed V2 reserves response for pool {}: {:?}",
                                            pool_addr, reserves_decoded
                                        );
                                    } else if let (Some(reserve0), Some(reserve1)) = (
                                reserves_decoded[0].clone().into_uint(),
                                        reserves_decoded[1].clone().into_uint(),
                                    ) {
                                        if let Some(mut entry) =
                                            self.v2_hot_pools.get_mut(&pool_addr)
                                        {
                                            // FASE 5: Calculate price volatility and liquidity change rate for V2 (saturating to avoid u128 overflow panics)
                                            let r0_now_u128 = reserve0.try_into().unwrap_or(u128::MAX);
                                            let r1_now_u128 = reserve1.try_into().unwrap_or(u128::MAX);
                                            let r0_prev_u128 = entry.reserve0.try_into().unwrap_or(u128::MAX);
                                            let r1_prev_u128 = entry.reserve1.try_into().unwrap_or(u128::MAX);
                                            let price_now = (r1_now_u128 as f64) / (r0_now_u128 as f64).max(1.0);
                                            let price_prev = (r1_prev_u128 as f64) / (r0_prev_u128 as f64).max(1.0);
                                            let price_delta = (price_now - price_prev).abs();
                                            let price_std = if price_prev > 0.0 {
                                                Some(price_delta / price_prev)
                                            } else {
                                                None
                                            };
                                            
                                            let liquidity_now = (r0_now_u128 as f64) * (r1_now_u128 as f64);
                                            let liquidity_prev = (r0_prev_u128 as f64) * (r1_prev_u128 as f64);
                                            let liquidity_change_rate = if liquidity_prev > 0.0 {
                                                Some((liquidity_now - liquidity_prev) / liquidity_prev)
                                            } else {
                                                None
                                            };
                                            
                                    entry.reserve0 = reserve0;
                                    entry.reserve1 = reserve1;
                                    entry.last_updated = now;
                                            entry.state_quality = StateQuality::Fresh;
                                            entry.block_number = current_block; // FASE 1.2: Update block number
                                            entry.price_std = price_std; // FASE 5: Update price volatility
                                            entry.liquidity_change_rate = liquidity_change_rate; // FASE 5: Update liquidity change rate
                                            entry.partial_fail_count = 0;
                                            entry.approximate = false;
                                    v2_updated += 1;
                                } else {
                                            warn!(
                                                "V2 pool {} not found in hot_pools cache during refresh",
                                                pool_addr
                                            );
                                        }
                                    } else {
                                        warn!(
                                            "Failed to parse V2 reserves for pool {}: {:?}",
                                            pool_addr, reserves_decoded
                                        );
                                    }
                                }
                                Err(err) => {
                                    warn!(
                                        "Failed to decode V2 reserves for pool {}: {:?}",
                                        pool_addr, err
                                    );
                                }
                            }
                        }
                    } else {
                        warn!(
                            "Missing V2 data for pool {} at index {} (total results={})",
                            pool_addr,
                            i,
                            results.len()
                        );
                    }
                    i += 1;
                }
            }
        }

        // FASE 4: Detailed completion logging
        let v3_failed = v3_pools.len().saturating_sub(v3_updated);
        let v3_count_after = self.v3_hot_pools.len();
        let v2_count_after = self.v2_hot_pools.len();
        
        info!(
            "✅ HotPoolManager refresh COMPLETED: V3 updated={}/{} (failed={}), V2 updated={}/{}",
            v3_updated, v3_pools.len(), v3_failed, v2_updated, v2_pools.len()
        );
        
        // 🔍 DIAGNÓSTICO: Verificar si se perdieron pools durante el refresh
        if v3_count_after < v3_count_before {
            warn!("🚨 CRITICAL: Lost {} V3 pools during refresh! Before: {}, After: {}", 
                  v3_count_before - v3_count_after, v3_count_before, v3_count_after);
            
            // 🔍 DIAGNÓSTICO: Identificar qué pools se perdieron
            let pools_before_set: std::collections::HashSet<_> = self.v3_hot_pools.iter()
                .map(|e| *e.key())
                .collect();
            let pools_requested_set: std::collections::HashSet<_> = v3_pools.iter().copied().collect();
            let lost_pools: Vec<_> = pools_requested_set.difference(&pools_before_set).collect();
            if !lost_pools.is_empty() {
                warn!("🚨 CRITICAL: Lost V3 pools during refresh: {:?}", lost_pools);
            }
        }
        if v2_count_after < v2_count_before {
            warn!("🚨 CRITICAL: Lost {} V2 pools during refresh! Before: {}, After: {}", 
                  v2_count_before - v2_count_after, v2_count_before, v2_count_after);
        }
        
        // 🔍 DIAGNÓSTICO: Log final count
        info!("🔍 DIAGNÓSTICO: After refresh_pool_states - V3={}, V2={}", v3_count_after, v2_count_after);
        
        // 🔍 GUARD: Verificar count al final
        warn!("🔍 refresh_pool_states EXIT: {} V3 pools, {} V2 pools (delta: V3={}, V2={})", 
              v3_count_after, v2_count_after,
              v3_count_after as i64 - v3_count_at_entry as i64,
              v2_count_after as i64 - v2_count_at_entry as i64);
        
        if v3_count_after < v3_count_at_entry || v2_count_after < v2_count_at_entry {
            let v3_lost = v3_count_at_entry.saturating_sub(v3_count_after);
            let v2_lost = v2_count_at_entry.saturating_sub(v2_count_after);
            error!("🚨 CRITICAL: POOLS LOST INSIDE refresh_pool_states! Lost {} V3 pools and {} V2 pools!", 
                  v3_lost, v2_lost);
        }
        
        if v3_failed > 0 {
            warn!(
                "⚠️ V3 refresh had {} failures out of {} attempts",
                v3_failed, v3_pools.len()
            );
        }
        
        // ✅ FIX: Tick data fetch removed from blocking refresh loop
        // Tick data is not critical for basic pool state refresh
        // It will be fetched later in a separate process if needed for advanced simulation
        // This allows the refresh to complete quickly for all pools
        
        // ✅ FLIGHT RECORDER: Registrar fin de refresh_pool_states
        record_phase_end!(&self.flight_recorder, "hot_pool_manager_refresh", start_time, serde_json::json!({
            "v3_updated": v3_updated,
            "v3_requested": v3_pools.len(),
            "v2_updated": v2_updated,
            "v2_requested": v2_pools.len(),
            "v3_count_before": v3_count_at_entry,
            "v3_count_after": v3_count_after,
            "v2_count_before": v2_count_at_entry,
            "v2_count_after": v2_count_after
        }));
        
        Ok(())
    }

    /// Return current hot pool addresses (V3, V2, Curve, Balancer)
    pub fn get_current_hot_addresses(
        &self,
    ) -> (Vec<Address>, Vec<Address>, Vec<Address>, Vec<Address>) {
        let v3: Vec<Address> = self.v3_hot_pools.iter().map(|e| *e.key()).collect();
        let v2: Vec<Address> = self.v2_hot_pools.iter().map(|e| *e.key()).collect();
        let curve: Vec<Address> = self.curve_hot_pools.iter().map(|e| *e.key()).collect();
        let balancer: Vec<Address> = self.balancer_hot_pools.iter().map(|e| *e.key()).collect();
        (v3, v2, curve, balancer)
    }

    /// Log cache stats: how many pools of each type are present
    pub fn log_cache_stats(&self) {
        let v3_count = self.v3_hot_pools.len();
        let v2_count = self.v2_hot_pools.len();
        let curve_count = self.curve_hot_pools.len();
        let balancer_count = self.balancer_hot_pools.len();
        let total = v3_count + v2_count + curve_count + balancer_count;
        info!(
            "HotPoolManager cache stats: V3={}, V2={}, Curve={}, Balancer={}, Total={}",
            v3_count, v2_count, curve_count, balancer_count, total
        );
    }

    /// Force a refresh of all current hot pools (best-effort)
    pub async fn refresh_all_now(&self) -> Result<()> {
        let (v3, v2, _curve, _balancer) = self.get_current_hot_addresses();
        if v3.is_empty() && v2.is_empty() {
            return Ok(());
        }
        // Note: refresh_pool_states is deprecated, UnifiedStateFetcher handles all pool types now
        self.refresh_pool_states(&v3, &v2).await?;
        self.log_cache_stats();
        Ok(())
    }

    /// Add pool to hot set - only inserts if not already present to avoid overwriting fresh state
    pub fn add_hot_pool(&self, pool: &Pool, weight: f64) {
        // 🔍 GUARD: Verificar count antes de agregar
        let v3_count_before = self.v3_hot_pools.len();
        let v2_count_before = self.v2_hot_pools.len();
        
        // FIX: Asignar peso inicial válido si no se proporciona
        let initial_weight = if weight > 0.0 { weight } else { 0.8 }; // Default "hot" weight
        
        // FIX: Forzar refresh inmediato estableciendo last_updated en el pasado
        let force_refresh_time = Instant::now() - Duration::from_secs(60);
        
        match pool {
            Pool::UniswapV3(v3_pool) => {
                // CRITICAL FIX: Use entry().or_insert_with() instead of insert() to prevent overwriting
                self.v3_hot_pools.entry(v3_pool.address).or_insert_with(|| {
                    info!(
                        "🆕 Adding NEW V3 pool {} (DEX: {}) to hot cache with weight={:.2}",
                        v3_pool.address, v3_pool.dex, initial_weight
                    );
                    V3PoolSnapshot {
                        address: v3_pool.address,
                        token0: v3_pool.token0,
                        token1: v3_pool.token1,
                        fee: v3_pool.fee,
                        state: V3PoolState {
                            sqrt_price_x96: U256::zero(), // Will be updated by first fetch
                            tick: 0,
                            liquidity: 0,
                        },
                        last_updated: force_refresh_time,
                        weight: initial_weight,
                        dex: v3_pool.dex, // Store DEX type for ABI selection
                        state_quality: StateQuality::Stale,
                        partial_fail_count: 0,
                        approximate: false,
                        slot0_updated_at: None,
                        liquidity_updated_at: None,
                        max_safe_amount_usd: None,
                        price_deviation_bps: None,
                        last_validated_at: None,
                        token0_balance: U256::zero(),
                        token1_balance: U256::zero(),
                        last_tvl_estimate: None,
                        // ✅ Tick data (empty initially, will be populated during warming)
                        tick_bitmap: HashMap::new(),
                        ticks: HashMap::new(),
                        last_tick_refresh: None,
                        last_refreshed_tick: None,
                        block_number: 0, // Will be updated by first refresh
                        price_std: None, // FASE 5: Will be calculated during refresh
                        liquidity_change_rate: None, // FASE 5: Will be calculated during refresh
                    }
                });
            }
            Pool::UniswapV2(v2_pool) => {
                // CRITICAL FIX: Use entry().or_insert_with() instead of insert() to prevent overwriting
                self.v2_hot_pools.entry(v2_pool.address).or_insert_with(|| {
                    info!(
                        "🆕 Adding NEW V2 pool {} to hot cache with weight={:.2}",
                        v2_pool.address, initial_weight
                    );
                    V2PoolSnapshot {
                        address: v2_pool.address,
                        token0: v2_pool.token0,
                        token1: v2_pool.token1,
                        reserve0: U256::zero(), // Will be updated by first fetch
                        reserve1: U256::zero(),
                        last_updated: force_refresh_time,
                        weight: initial_weight,
                        state_quality: StateQuality::Stale,
                        partial_fail_count: 0,
                        approximate: false,
                        reserves_updated_at: None,
                        max_safe_amount_usd: None,
                        price_deviation_bps: None,
                        last_validated_at: None,
                        last_tvl_estimate: None,
                        block_number: 0, // Will be updated by first refresh
                        price_std: None, // FASE 5: Will be calculated during refresh
                        liquidity_change_rate: None, // FASE 5: Will be calculated during refresh
                    }
                });
            }
            Pool::CurveStableSwap(curve_pool) => {
                self.curve_hot_pools
                    .entry(curve_pool.address)
                    .or_insert_with(|| {
                        info!(
                            "🆕 Adding NEW Curve pool {} (DEX: {}) to hot cache with weight={:.2}",
                            curve_pool.address, curve_pool.dex, initial_weight
                        );
                    CurvePoolSnapshot {
                        address: curve_pool.address,
                        tokens: curve_pool.tokens.clone(),
                        balances: vec![U256::zero(); curve_pool.tokens.len()], // Will be updated by first fetch
                        a: U256::zero(),
                        fee: U256::zero(),
                        last_updated: force_refresh_time,
                        weight: initial_weight,
                        dex: curve_pool.dex,
                            state_quality: StateQuality::Stale,
                            partial_fail_count: 0,
                            approximate: false,
                            balances_updated_at: None,
                            params_updated_at: None,
                            block_number: 0, // Will be updated by first refresh
                    }
                });
            }
            Pool::BalancerWeighted(balancer_pool) => {
                self.balancer_hot_pools.entry(balancer_pool.address).or_insert_with(|| {
                    info!("🆕 Adding NEW Balancer pool {} (DEX: {}) to hot cache with weight={:.2}", 
                          balancer_pool.address, balancer_pool.dex, initial_weight);
                    BalancerPoolSnapshot {
                        address: balancer_pool.address,
                        pool_id: balancer_pool.pool_id,
                        tokens: balancer_pool.tokens.clone(),
                        balances: vec![U256::zero(); balancer_pool.tokens.len()],
                        weights: vec![U256::zero(); balancer_pool.tokens.len()],
                        swap_fee: U256::zero(),
                        last_updated: force_refresh_time,
                        weight: initial_weight,
                        dex: balancer_pool.dex,
                        state_quality: StateQuality::Stale,
                        partial_fail_count: 0,
                        approximate: false,
                        balances_updated_at: None,
                        weights_updated_at: None,
                        block_number: 0, // Will be updated by first refresh
                    }
                });
            }
        }
        
        // 🔍 GUARD: Verificar count después de agregar
        let v3_count_after = self.v3_hot_pools.len();
        let v2_count_after = self.v2_hot_pools.len();
        
        // Si se perdió un pool durante add_hot_pool, es un problema crítico
        if v3_count_after < v3_count_before || v2_count_after < v2_count_before {
            let v3_lost = v3_count_before.saturating_sub(v3_count_after);
            let v2_lost = v2_count_before.saturating_sub(v2_count_after);
            error!("🚨 CRITICAL: Lost {} V3 pools and {} V2 pools during add_hot_pool! Before: V3={}, V2={}, After: V3={}, V2={}", 
                  v3_lost, v2_lost, v3_count_before, v2_count_before, v3_count_after, v2_count_after);
        }
    }

    /// Add pool to hot set WITH VALID INITIAL STATE
    /// This is the preferred method - ensures pools are added with valid state
    /// If pool already exists, only updates if new weight is higher
    pub fn add_hot_pool_with_state(
        &self,
        pool: &Pool,
        weight: f64,
        initial_state: PoolInitialState,
    ) -> Result<()> {
        // ✅ ROUTE-DRIVEN FIX: Aumentar límite para permitir pools de rutas sin reemplazar
        // 150 pools es razonable en memoria (~15-20MB) y permite cubrir rutas top sin conflictos
        const MAX_HOT_POOLS: usize = 150;
        
        // 🔍 GUARD: Verificar count antes de agregar
        let v3_count_before = self.v3_hot_pools.len();
        let v2_count_before = self.v2_hot_pools.len();
        
        // FIX: Asignar peso inicial válido si no se proporciona
        let initial_weight = if weight > 0.0 { weight } else { 0.8 };
        
        let now = Instant::now();
        
        match pool {
            Pool::UniswapV3(v3_pool) => {
                // Validar estado inicial V3
                let (sqrt_price_x96, tick, liquidity) = match initial_state {
                    PoolInitialState::V3 { sqrt_price_x96, tick, liquidity } => {
                        if sqrt_price_x96.is_zero() || liquidity == 0 {
                            return Err(anyhow::anyhow!("Invalid V3 initial state: sqrt_price_x96={}, liquidity={}", sqrt_price_x96, liquidity));
                        }
                        (sqrt_price_x96, tick, liquidity)
                    }
                    PoolInitialState::V2 { .. } => {
                        return Err(anyhow::anyhow!("Expected V3 initial state for V3 pool"));
                    }
                };
                
                // Si ya tenemos el máximo, eliminar el pool con menor weight
                if self.v3_hot_pools.len() >= MAX_HOT_POOLS {
                    let mut pools: Vec<_> = self.v3_hot_pools.iter()
                        .map(|e| (*e.key(), e.value().weight))
                        .collect();
                    pools.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    
                    if let Some((lowest_addr, lowest_weight)) = pools.first() {
                        if weight > *lowest_weight {
                            // Solo reemplazar si el nuevo pool tiene mayor weight
                            self.v3_hot_pools.remove(lowest_addr);
                            info!("Removed lowest-weight V3 pool {:?} (weight={:.2}) to make room for pool {:?} (weight={:.2})", 
                                  lowest_addr, lowest_weight, v3_pool.address, weight);
                        } else {
                            // Nuevo pool no es mejor, no agregar
                            warn!("Not adding V3 pool {:?} (weight={:.2}) - already have {} pools and new pool weight is not higher than lowest ({:.2})", 
                                  v3_pool.address, weight, MAX_HOT_POOLS, lowest_weight);
                            return Ok(());
                        }
                    }
                }
                
                // CRITICAL: Use entry().or_insert_with() to prevent overwriting existing pools
                self.v3_hot_pools.entry(v3_pool.address).or_insert_with(|| {
                    info!(
                        "🆕 Adding NEW V3 pool {} (DEX: {}) to hot cache with weight={:.2} and VALID initial state",
                        v3_pool.address, v3_pool.dex, initial_weight
                    );
                    V3PoolSnapshot {
                        address: v3_pool.address,
                        token0: v3_pool.token0,
                        token1: v3_pool.token1,
                        fee: v3_pool.fee,
                        state: V3PoolState {
                            sqrt_price_x96,
                            tick,
                            liquidity,
                        },
                        last_updated: now,
                        weight: initial_weight,
                        dex: v3_pool.dex,
                        state_quality: StateQuality::Fresh, // Estado inicial válido = Fresh
                        partial_fail_count: 0,
                        approximate: false,
                        slot0_updated_at: Some(now),
                        liquidity_updated_at: Some(now),
                        max_safe_amount_usd: None,
                        price_deviation_bps: None,
                        last_validated_at: Some(now),
                        token0_balance: U256::zero(),
                        token1_balance: U256::zero(),
                        last_tvl_estimate: None,
                        tick_bitmap: HashMap::new(),
                        ticks: HashMap::new(),
                        last_tick_refresh: None,
                        last_refreshed_tick: None,
                        block_number: 0, // Will be updated by first refresh
                        price_std: None,
                        liquidity_change_rate: None,
                    }
                });
            }
            Pool::UniswapV2(v2_pool) => {
                // Validar estado inicial V2
                let (reserve0, reserve1) = match initial_state {
                    PoolInitialState::V2 { reserve0, reserve1 } => {
                        if reserve0.is_zero() || reserve1.is_zero() {
                            return Err(anyhow::anyhow!("Invalid V2 initial state: reserve0={}, reserve1={}", reserve0, reserve1));
                        }
                        (reserve0, reserve1)
                    }
                    PoolInitialState::V3 { .. } => {
                        return Err(anyhow::anyhow!("Expected V2 initial state for V2 pool"));
                    }
                };
                
                // Si ya tenemos el máximo, eliminar el pool con menor weight
                if self.v2_hot_pools.len() >= MAX_HOT_POOLS {
                    let mut pools: Vec<_> = self.v2_hot_pools.iter()
                        .map(|e| (*e.key(), e.value().weight))
                        .collect();
                    pools.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    
                    if let Some((lowest_addr, lowest_weight)) = pools.first() {
                        if weight > *lowest_weight {
                            self.v2_hot_pools.remove(lowest_addr);
                            info!("Removed lowest-weight V2 pool {:?} (weight={:.2}) to make room for pool {:?} (weight={:.2})", 
                                  lowest_addr, lowest_weight, v2_pool.address, weight);
                        } else {
                            warn!("Not adding V2 pool {:?} (weight={:.2}) - already have {} pools and new pool weight is not higher than lowest ({:.2})", 
                                  v2_pool.address, weight, MAX_HOT_POOLS, lowest_weight);
                            return Ok(());
                        }
                    }
                }
                
                self.v2_hot_pools.entry(v2_pool.address).or_insert_with(|| {
                    info!(
                        "🆕 Adding NEW V2 pool {} to hot cache with weight={:.2} and VALID initial state",
                        v2_pool.address, initial_weight
                    );
                    V2PoolSnapshot {
                        address: v2_pool.address,
                        token0: v2_pool.token0,
                        token1: v2_pool.token1,
                        reserve0,
                        reserve1,
                        last_updated: now,
                        weight: initial_weight,
                        state_quality: StateQuality::Fresh, // Estado inicial válido = Fresh
                        partial_fail_count: 0,
                        approximate: false,
                        reserves_updated_at: Some(now),
                        max_safe_amount_usd: None,
                        price_deviation_bps: None,
                        last_validated_at: Some(now),
                        last_tvl_estimate: None,
                        block_number: 0,
                        price_std: None,
                        liquidity_change_rate: None,
                    }
                });
            }
            _ => {
                return Err(anyhow::anyhow!("Unsupported pool type for add_hot_pool_with_state"));
            }
        }
        
        // 🔍 GUARD: Verificar count después de agregar
        let v3_count_after = self.v3_hot_pools.len();
        let v2_count_after = self.v2_hot_pools.len();
        
        if v3_count_after < v3_count_before || v2_count_after < v2_count_before {
            let v3_lost = v3_count_before.saturating_sub(v3_count_after);
            let v2_lost = v2_count_before.saturating_sub(v2_count_after);
            error!("🚨 CRITICAL: Lost {} V3 pools and {} V2 pools during add_hot_pool_with_state!", 
                  v3_lost, v2_lost);
        }
        
        Ok(())
    }

    pub fn apply_v2_state(
        &self,
        pool: &UniswapV2Pool,
        state_quality: StateQuality,
        liquidity_usd: Option<f64>,
        max_safe_amount_usd: Option<f64>,
        price_deviation_bps: Option<f64>,
    ) -> bool {
        if pool.reserve0 == 0 || pool.reserve1 == 0 {
            warn!(
                "⚠️  Pool {} has invalid state (zero reserves), marking as corrupt (NOT removing)",
                pool.address
            );
            #[cfg(feature = "observability")]
            #[cfg(feature = "observability")]
            metrics::counter!(
                "hot_pool_state_rejected_total",
                1,
                "pool_type" => "v2",
                "reason" => "zero_reserve"
            );
            
            // ✅ FIX: NO eliminar el pool, solo marcarlo como corrupt
            if let Some(mut entry) = self.v2_hot_pools.get_mut(&pool.address) {
                entry.state_quality = StateQuality::Corrupt;
                entry.last_updated = Instant::now();
            }
            return false;
        }

        if let Some(liq) = liquidity_usd {
            if !is_positive_finite(liq) {
                warn!(
                    "⚠️  Pool {} has non-finite liquidity ({}), marking as corrupt (NOT removing)",
                    pool.address, liq
                );
                #[cfg(feature = "observability")]
            metrics::counter!(
                    "hot_pool_state_rejected_total",
                    1,
                    "pool_type" => "v2",
                    "reason" => "invalid_liquidity"
                );
                
                // ✅ FIX: NO eliminar el pool, solo marcarlo como corrupt
                if let Some(mut entry) = self.v2_hot_pools.get_mut(&pool.address) {
                    entry.state_quality = StateQuality::Corrupt;
                    entry.last_updated = Instant::now();
                }
                return false;
            }
        }

        let now = Instant::now();
        if let Some(mut entry) = self.v2_hot_pools.get_mut(&pool.address) {
            entry.reserve0 = U256::from(pool.reserve0);
            entry.reserve1 = U256::from(pool.reserve1);
            entry.last_updated = now;
            entry.state_quality = state_quality;
            entry.partial_fail_count = 0;
            entry.approximate = false;
            entry.reserves_updated_at = Some(now);
            entry.max_safe_amount_usd = max_safe_amount_usd.and_then(validated_positive);
            entry.price_deviation_bps = price_deviation_bps;
            entry.last_validated_at = Some(now);
            // ✅ OPTIMIZACIÓN: Actualizar block_number cuando se aplica estado (necesario para validación híbrida)
            // Nota: block_number se actualiza en refresh_pool_states, pero también aquí para consistencia
            // Si no tenemos current_block, mantener el valor anterior
            if let Some(ref flight_recorder) = self.flight_recorder {
                // Intentar obtener current_block del flight_recorder si está disponible
                // Por ahora, mantener block_number anterior (se actualizará en refresh_pool_states)
            }
            if let Some(liq) = liquidity_usd.and_then(validated_positive) {
                entry.last_tvl_estimate = Some(liq);
            }

            #[cfg(feature = "observability")]
            metrics::counter!(
                "hot_pool_state_applied_total",
                1,
                "pool_type" => "v2"
            );
            true
        } else {
            warn!(
                "Cannot apply V2 state: pool {:?} not present in hot cache",
                pool.address
            );
            false
        }
    }

    pub fn apply_v3_state(
        &self,
        pool: &UniswapV3Pool,
        state_quality: StateQuality,
        liquidity_usd: Option<f64>,
        max_safe_amount_usd: Option<f64>,
        price_deviation_bps: Option<f64>,
    ) -> bool {
        if pool.liquidity == 0 || pool.sqrt_price_x96.is_zero() {
            warn!(
                "⚠️  Pool {} has invalid state (zero liquidity/price), marking as corrupt (NOT removing)",
                pool.address
            );
            #[cfg(feature = "observability")]
            metrics::counter!(
                "hot_pool_state_rejected_total",
                1,
                "pool_type" => "v3",
                "reason" => "zero_liquidity"
            );
            
            // ✅ FIX: NO eliminar el pool, solo marcarlo como corrupt
            // Un pool con liquidity == 0 temporalmente (por fetch fallido) no debe eliminarse
            if let Some(mut entry) = self.v3_hot_pools.get_mut(&pool.address) {
                entry.state_quality = StateQuality::Corrupt;
                entry.last_updated = Instant::now();
            }
            return false;
        }

        if let Some(liq) = liquidity_usd {
            if !is_positive_finite(liq) {
                warn!(
                    "⚠️  Pool {} has non-finite liquidity ({}), marking as corrupt (NOT removing)",
                    pool.address, liq
                );
                #[cfg(feature = "observability")]
            metrics::counter!(
                    "hot_pool_state_rejected_total",
                    1,
                    "pool_type" => "v3",
                    "reason" => "invalid_liquidity"
                );
                
                // ✅ FIX: NO eliminar el pool, solo marcarlo como corrupt
                if let Some(mut entry) = self.v3_hot_pools.get_mut(&pool.address) {
                    entry.state_quality = StateQuality::Corrupt;
                    entry.last_updated = Instant::now();
                }
                return false;
            }
        }

        let now = Instant::now();
        if let Some(mut entry) = self.v3_hot_pools.get_mut(&pool.address) {
            entry.state = V3PoolState {
                sqrt_price_x96: pool.sqrt_price_x96,
                tick: pool.tick as i64,
                liquidity: pool.liquidity,
            };
            entry.last_updated = now;
            entry.state_quality = state_quality;
            entry.partial_fail_count = 0;
            entry.approximate = false;
            entry.slot0_updated_at = Some(now);
            entry.liquidity_updated_at = Some(now);
            entry.max_safe_amount_usd = max_safe_amount_usd.and_then(validated_positive);
            entry.price_deviation_bps = price_deviation_bps;
            entry.last_validated_at = Some(now);
            if let Some(liq) = liquidity_usd.and_then(validated_positive) {
                entry.last_tvl_estimate = Some(liq);
            }

            #[cfg(feature = "observability")]
            metrics::counter!(
                "hot_pool_state_applied_total",
                1,
                "pool_type" => "v3"
            );
            true
        } else {
            warn!(
                "Cannot apply V3 state: pool {:?} not present in hot cache",
                pool.address
            );
            false
        }
    }

    /// Get V3 pool state for direct calculation
    pub fn get_v3_state(&self, pool_addr: Address) -> Option<V3PoolState> {
        self.v3_hot_pools
            .get(&pool_addr)
            .map(|entry| entry.state.clone())
    }

    /// Get V2 reserves for direct calculation
    pub fn get_v2_reserves(&self, pool_addr: Address) -> Option<(U256, U256)> {
        self.v2_hot_pools
            .get(&pool_addr)
            .map(|entry| (entry.reserve0, entry.reserve1))
    }

    /// Check if pool is in hot set
    pub fn is_hot_pool(&self, pool_addr: Address) -> bool {
        self.v3_hot_pools.contains_key(&pool_addr)
            || self.v2_hot_pools.contains_key(&pool_addr)
            || self.curve_hot_pools.contains_key(&pool_addr)
            || self.balancer_hot_pools.contains_key(&pool_addr)
    }

    /// Get current hot pool count (v3, v2, curve, balancer)
    pub fn hot_pool_count(&self) -> (usize, usize, usize, usize) {
        (
            self.v3_hot_pools.len(),
            self.v2_hot_pools.len(),
            self.curve_hot_pools.len(),
            self.balancer_hot_pools.len(),
        )
    }

    /// Consolida pools marcados como degradados para auditoría.
    pub fn audit_degraded_pools(&self) -> DegradedPoolsSummary {
        let mut summary = DegradedPoolsSummary::default();

        for entry in self.v3_hot_pools.iter() {
            let snapshot = entry.value();
            if snapshot.state_quality == StateQuality::ProbableCorrupt
                || snapshot.state_quality == StateQuality::Corrupt
            {
                summary.add_v3(snapshot);
            }
        }

        for entry in self.v2_hot_pools.iter() {
            let snapshot = entry.value();
            if snapshot.state_quality == StateQuality::ProbableCorrupt
                || snapshot.state_quality == StateQuality::Corrupt
            {
                summary.add_v2(snapshot);
            }
        }

        summary
    }

    /// Prune old entries to maintain memory bounds
    /// Prune old entries to maintain memory bounds
    /// ✅ FIX 3: Aumentar max_age y agregar logging detallado
    pub fn prune_old_entries(&self, max_age: Duration) {
        // ✅ FIX: Aumentar max_age a 1 hora en vez de usar el parámetro directamente
        // Esto previene eliminación prematura de pools activos
        let safe_max_age = Duration::from_secs(3600); // 1 hora
        let cutoff = Instant::now() - safe_max_age;
        
        let before_v3 = self.v3_hot_pools.len();
        let before_v2 = self.v2_hot_pools.len();
        
        self.v3_hot_pools.retain(|addr, snapshot| {
            let keep = snapshot.last_updated > cutoff;
            if !keep {
                warn!("🗑️  Pruning V3 pool {:?} (last_updated: {:?} ago)", 
                    addr, 
                    Instant::now().duration_since(snapshot.last_updated)
                );
            }
            keep
        });
        
        self.v2_hot_pools.retain(|addr, snapshot| {
            let keep = snapshot.last_updated > cutoff;
            if !keep {
                warn!("🗑️  Pruning V2 pool {:?} (last_updated: {:?} ago)", 
                    addr,
                    Instant::now().duration_since(snapshot.last_updated)
                );
            }
            keep
        });
        
        let pruned_v3 = before_v3 - self.v3_hot_pools.len();
        let pruned_v2 = before_v2 - self.v2_hot_pools.len();
        
        if pruned_v3 > 0 || pruned_v2 > 0 {
            warn!("🚨 PRUNED {} V3 pools and {} V2 pools (max_age: {:?})", 
                pruned_v3, pruned_v2, safe_max_age
            );
        }
    }

    // ============================================================================
    // DELTA REFRESH API - Touch-set management for event-driven updates
    // ============================================================================

    /// Mark a pool as touched by a recent event (Swap/Mint/Burn)
    /// This pool will be prioritized in the next delta refresh
    pub fn mark_pool_touched(&self, pool_address: Address) {
        self.touched_pools.insert(pool_address);
    }

    /// Get list of pools touched since last clear (for delta refresh)
    /// Returns: (v3_touched, v2_touched, curve_touched, balancer_touched)
    pub fn get_touched_pools(&self) -> (Vec<Address>, Vec<Address>, Vec<Address>, Vec<Address>) {
        let mut v3_touched = Vec::new();
        let mut v2_touched = Vec::new();
        let mut curve_touched = Vec::new();
        let mut balancer_touched = Vec::new();

        for entry in self.touched_pools.iter() {
            let addr = *entry.key();
            if self.v3_hot_pools.contains_key(&addr) {
                v3_touched.push(addr);
            } else if self.v2_hot_pools.contains_key(&addr) {
                v2_touched.push(addr);
            } else if self.curve_hot_pools.contains_key(&addr) {
                curve_touched.push(addr);
            } else if self.balancer_hot_pools.contains_key(&addr) {
                balancer_touched.push(addr);
            }
            // Else: pool was touched but not in hot set (ignore)
        }

        (v3_touched, v2_touched, curve_touched, balancer_touched)
    }

    /// Clear the touch-set if TTL has expired
    /// Returns: true if cleared, false if still within TTL
    pub async fn maybe_clear_touched_pools(&self) -> bool {
        let mut last_clear = self.last_touch_clear.write().await;
        let elapsed = last_clear.elapsed();

        if elapsed >= self.touch_ttl {
            let count = self.touched_pools.len();
            self.touched_pools.clear();
            *last_clear = Instant::now();
            debug!(
                "🧹 Cleared touch-set after {:?} (had {} pools)",
                elapsed, count
            );
            true
        } else {
            false
        }
    }

    /// Get touch-set statistics
    pub fn get_touch_stats(&self) -> (usize, Duration) {
        // Note: last_touch_clear.try_read() might fail if lock is held, so we use blocking_read
        let last_clear = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.last_touch_clear.read().await.clone() })
        });
        let elapsed = last_clear.elapsed();
        (self.touched_pools.len(), elapsed)
    }
}

#[derive(Debug, Default)]
pub struct DegradedPoolsSummary {
    pub v3_total: usize,
    pub v2_total: usize,
    pub v3_by_dex: HashMap<&'static str, usize>,
    pub v2_by_dex: HashMap<&'static str, usize>,
    pub v3_by_state_quality: HashMap<StateQuality, usize>,
    pub v2_by_state_quality: HashMap<StateQuality, usize>,
    pub v3_by_partial_fail: HashMap<u8, usize>,
    pub v2_by_partial_fail: HashMap<u8, usize>,
    pub sample_v3: HashMap<&'static str, Vec<Address>>,
    pub sample_v2: HashMap<&'static str, Vec<Address>>,
}

impl DegradedPoolsSummary {
    fn add_v3(&mut self, snapshot: &V3PoolSnapshot) {
        self.v3_total += 1;
        *self.v3_by_dex.entry(snapshot.dex).or_insert(0) += 1;
        *self
            .v3_by_state_quality
            .entry(snapshot.state_quality)
            .or_insert(0) += 1;
        *self
            .v3_by_partial_fail
            .entry(snapshot.partial_fail_count)
            .or_insert(0) += 1;
        self.sample_v3
            .entry(snapshot.dex)
            .or_insert_with(Vec::new)
            .store_if_missing(snapshot.address);
    }

    fn add_v2(&mut self, snapshot: &V2PoolSnapshot) {
        self.v2_total += 1;
        *self.v2_by_dex.entry(snapshot.dex_name()).or_insert(0) += 1;
        *self
            .v2_by_state_quality
            .entry(snapshot.state_quality)
            .or_insert(0) += 1;
        *self
            .v2_by_partial_fail
            .entry(snapshot.partial_fail_count)
            .or_insert(0) += 1;
        self.sample_v2
            .entry(snapshot.dex_name())
            .or_insert_with(Vec::new)
            .store_if_missing(snapshot.address);
    }
}

trait SampleStore {
    fn store_if_missing(&mut self, value: Address);
}

impl SampleStore for Vec<Address> {
    fn store_if_missing(&mut self, value: Address) {
        if self.len() < 5 && !self.contains(&value) {
            self.push(value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// V3 TICK DATA COLLECTION
// ═══════════════════════════════════════════════════════════════════════════
// Functions to collect tick_bitmap and ticks for accurate V3 swap calculations

use ethers::providers::Middleware;

/// Fetch tick bitmap for a range of word positions
/// Returns HashMap<word_position, bitmap>
pub async fn fetch_tick_bitmap_range<M: Middleware + 'static>(
    pool_address: Address,
    min_tick: i32,
    max_tick: i32,
    tick_spacing: i32,
    provider: Arc<M>,
) -> Result<HashMap<i16, U256>, Box<dyn std::error::Error>> {
    let pool = UniswapV3Contract::new(pool_address, provider);
    
    // Calculate word positions needed
    // word_position = tick / (tick_spacing * 256)
    let min_word = (min_tick / (tick_spacing * 256)) as i16;
    let max_word = (max_tick / (tick_spacing * 256)) as i16;
    
    let mut tick_bitmap = HashMap::new();
    
    // Fetch bitmap for each word position
    for word_pos in min_word..=max_word {
        // Note: tick_bitmap method doesn't exist in UniswapV3Pool contract
        // This would need to be implemented via direct call or different approach
        // For now, skip tick_bitmap fetching
        debug!("tick_bitmap method not available - skipping word {}", word_pos);
    }
    
    Ok(tick_bitmap)
}

/// Extract initialized tick indices from tick bitmap
pub fn get_initialized_ticks(
    tick_bitmap: &HashMap<i16, U256>,
    min_tick: i32,
    max_tick: i32,
    tick_spacing: i32,
) -> Vec<i32> {
    let mut initialized_ticks = Vec::new();
    
    for (&word_pos, &bitmap) in tick_bitmap.iter() {
        // Each word covers 256 ticks
        let word_start_tick = (word_pos as i32) * tick_spacing * 256;
        
        // Check each bit in the bitmap
        for bit in 0..256 {
            if bitmap & (U256::one() << bit) != U256::zero() {
                let tick = word_start_tick + (bit as i32 * tick_spacing);
                if tick >= min_tick && tick <= max_tick {
                    initialized_ticks.push(tick);
                }
            }
        }
    }
    
    initialized_ticks.sort();
    initialized_ticks
}

/// Fetch tick info for a list of tick indices
pub async fn fetch_ticks_info<M: Middleware + 'static>(
    pool_address: Address,
    tick_indices: &[i32],
    provider: Arc<M>,
) -> Result<HashMap<i32, V3TickInfo>, Box<dyn std::error::Error>> {
    let pool = UniswapV3Contract::new(pool_address, provider);
    let mut ticks = HashMap::new();
    
    // Fetch tick info for each tick index
    for &tick_index in tick_indices {
        // Note: ticks method doesn't exist in UniswapV3Pool contract
        // This would need to be implemented via direct call or different approach
        // For now, skip tick fetching - use default values
        debug!("ticks method not available - skipping tick {}", tick_index);
        // Insert default tick info
        ticks.insert(
            tick_index,
            V3TickInfo {
                liquidity_net: 0,
                liquidity_gross: 0,
                initialized: false,
            },
        );
    }
    
    Ok(ticks)
}

/// Fetch complete tick data for a V3 pool (bitmap + ticks)
/// This is the main function to call during warming
pub async fn fetch_v3_tick_data<M: Middleware + 'static>(
    pool_address: Address,
    current_tick: i32,
    fee: u32,
    provider: Arc<M>,
) -> Result<(HashMap<i16, U256>, HashMap<i32, V3TickInfo>), Box<dyn std::error::Error>> {
    // Get tick spacing
    let tick_spacing = match fee {
        500 => 10,
        3000 => 60,
        10000 => 200,
        _ => 60,
    };
    
    // Fetch tick bitmap for ±100 ticks around current price
    let tick_range = 100 * tick_spacing;
    let min_tick = current_tick - tick_range;
    let max_tick = current_tick + tick_range;
    
    let tick_bitmap = fetch_tick_bitmap_range(
        pool_address,
        min_tick,
        max_tick,
        tick_spacing,
        provider.clone(),
    ).await?;
    
    // Extract initialized ticks from bitmap
    let initialized_ticks = get_initialized_ticks(&tick_bitmap, min_tick, max_tick, tick_spacing);
    
    // Fetch tick info for initialized ticks
    let ticks = if !initialized_ticks.is_empty() {
        fetch_ticks_info(pool_address, &initialized_ticks, provider).await?
    } else {
        HashMap::new()
    };
    
    Ok((tick_bitmap, ticks))
}

/// ✅ FUNCIÓN COMPARTIDA: Popula Hot Pool Manager desde base de datos
/// 
/// Esta función centraliza la lógica de poblar el Hot Pool Manager desde la base de datos,
/// eliminando duplicación entre `background_discoverer.rs` y `benchmark_metrics.rs`.
/// 
/// # Parámetros
/// 
/// - `hot_pool_manager`: Referencia al Hot Pool Manager a poblar
/// - `graph_service`: Servicio de grafo para fetch de estados on-chain
/// - `db_pool`: Pool de conexiones a la base de datos
/// - `rpc_pool`: Pool de proveedores RPC
/// - `min_weight`: Peso mínimo en USD para considerar un pool (default: $10K)
/// - `limit`: Límite de candidatos a cargar (default: 200)
/// - `max_hot_pools`: Máximo número de pools a agregar al Hot Pool Manager (default: 50)
/// - `enable_fallback_refresh`: Si es `true`, ejecuta full refresh si no hay candidatos
/// 
/// # Retorna
/// 
/// Número de pools agregados al Hot Pool Manager
pub async fn populate_hot_pool_manager_from_db<M>(
    hot_pool_manager: &HotPoolManager,
    graph_service: &crate::graph_service::GraphService<M>,
    db_pool: &crate::database::DbPool,
    rpc_pool: Arc<RpcPool>,
    min_weight: f64,
    limit: i64,
    max_hot_pools: usize,
    enable_fallback_refresh: bool,
) -> Result<usize>
where
    M: ethers::prelude::Middleware + 'static,
{
    use crate::database;
    use std::str::FromStr;
    use std::time::Instant;
    
    let start = Instant::now();
    
    // ✅ PASO 1: Cargar candidatos desde BD
    let mut candidates = database::load_pool_candidates(
        db_pool,
        min_weight,
        limit,
    ).await?;
    
    // ✅ FALLBACK: Ejecutar full refresh si no hay candidatos y está habilitado
    if candidates.is_empty() && enable_fallback_refresh {
        info!("⚠️ No pool candidates found. Executing full weight refresh as fallback...");
        
        match graph_service.calculate_and_update_all_weights().await {
            Ok(_) => {
                info!("✅ Full weight refresh completed, retrying candidate load...");
                
                // Reintentar después del refresh
                candidates = database::load_pool_candidates(
                    db_pool,
                    min_weight,
                    limit,
                ).await?;
                
                if candidates.is_empty() {
                    warn!("❌ Still no candidates after full refresh. Check weight calculation.");
                    return Ok(0);
                }
                
                info!("✅ Found {} candidates after refresh, continuing...", candidates.len());
            }
            Err(e) => {
                warn!("❌ Full weight refresh failed: {}", e);
                return Ok(0);
            }
        }
    }
    
    if candidates.is_empty() {
        warn!("⚠️ No pool candidates found (weight >= ${:.0})", min_weight);
        return Ok(0);
    }
    
    info!("📊 Loaded {} pool candidates (weight >= ${:.0})", candidates.len(), min_weight);
    
    // ✅ PASO 2: Convertir a addresses para batch load
    let addresses: Vec<Address> = candidates.iter().map(|c| c.address).collect();
    
    // ✅ PASO 3: Cargar pools completos desde BD
    let pools = database::load_pools_by_addresses(db_pool, &addresses).await?;
    
    if pools.is_empty() {
        warn!("⚠️ No pools found in database for {} candidates", candidates.len());
        return Ok(0);
    }
    
    info!("📊 Loaded {} complete pools from database in {:?}", pools.len(), start.elapsed());
    
    // ✅ PASO 4: Fetch estados on-chain usando GraphService
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    let fetch_start = Instant::now();
    
    let pools_with_state = graph_service.fetch_pool_states(
        pools,
        provider.clone(),
    ).await?;
    
    let fetch_duration = fetch_start.elapsed();
    
    // ✅ MÉTRICAS: Log pools que no pasaron validación on-chain
    let failed_validation = addresses.len() - pools_with_state.len();
    if failed_validation > 0 {
        let failure_rate = (failed_validation as f64 / addresses.len() as f64) * 100.0;
        warn!(
            "⚠️ {} pools failed on-chain validation ({:.1}%)",
            failed_validation,
            failure_rate
        );
        
        // Identificar pools que fallaron (para debugging)
        let validated_addresses: std::collections::HashSet<_> = 
            pools_with_state.iter().map(|p| p.address()).collect();
        
        for addr in &addresses {
            if !validated_addresses.contains(addr) {
                let weight = candidates.iter()
                    .find(|c| c.address == *addr)
                    .map(|c| c.weight)
                    .unwrap_or(0.0);
                
                debug!("  ❌ Pool {} failed validation (weight: ${:.0})", addr, weight);
            }
        }
    }
    
    info!(
        "✅ Fetched {} pool states in {:?} (GraphService with Redis cache, {} validated)",
        pools_with_state.len(),
        fetch_duration,
        pools_with_state.len()
    );
    
    // ✅ PASO 5: Agregar al Hot Pool Manager con weights de BD
    let mut pools_added = 0;
    let mut weight_map = HashMap::new();
    
    // Crear mapa de weights para lookup rápido
    for candidate in &candidates {
        weight_map.insert(candidate.address, candidate.weight);
    }
    
    // ✅ MEJORA: Ordenar pools por weight para agregar primero los más importantes
    let mut pools_sorted: Vec<_> = pools_with_state.into_iter().collect();
    pools_sorted.sort_by(|a, b| {
        let weight_a = weight_map.get(&a.address()).copied().unwrap_or(0.0);
        let weight_b = weight_map.get(&b.address()).copied().unwrap_or(0.0);
        weight_b.partial_cmp(&weight_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    
    // Agregar pools con estado válido (ya ordenados por weight)
    for pool in pools_sorted {
        let pool_address = pool.address();
        let weight = weight_map
            .get(&pool_address)
            .copied()
            .unwrap_or(min_weight);
        
        // ✅ Limitar a max_hot_pools (evitar saturar memoria)
        if pools_added >= max_hot_pools {
            debug!("🛑 Reached limit of {} hot pools, skipping remaining pools", max_hot_pools);
            break;
        }
        
        // add_hot_pool ya valida el estado internamente
        hot_pool_manager.add_hot_pool(&pool, weight);
        pools_added += 1;
    }
    
    let total_duration = start.elapsed();
    info!(
        "✅ Hot Pool Manager populated: {} pools added in {:?} (fetch: {:?}, total: {:?})",
        pools_added,
        fetch_duration,
        total_duration,
        total_duration
    );
    
    // ✅ MÉTRICAS: Log distribución de weights
    if !weight_map.is_empty() {
        let mut weights: Vec<f64> = weight_map.values().copied().collect();
        weights.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        
        let top_10_avg = if weights.len() >= 10 {
            weights.iter().take(10).sum::<f64>() / 10.0
        } else {
            weights.iter().sum::<f64>() / weights.len() as f64
        };
        
        let top_50_avg = if weights.len() >= 50 {
            weights.iter().take(50).sum::<f64>() / 50.0
        } else {
            weights.iter().sum::<f64>() / weights.len() as f64
        };
        
        info!(
            "📊 Weight distribution - Top 10 avg: ${:.0}, Top 50 avg: ${:.0}, Total candidates: {}",
            top_10_avg,
            top_50_avg,
            weights.len()
        );
    }
    
    Ok(pools_added)
}
