// JIT State Fetcher - Dynamic refresh just-in-time for route-driven pools
// This module implements the JIT Dynamic Refresh model: fetch pool states
// only for pools needed by top routes, eliminating HotPoolManager state management complexity.

use anyhow::Result;
use ethers::prelude::{Http, Provider};
use ethers::types::{Address, U256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use log::{info, warn, error, debug};
use hex;

use crate::contracts::{
    uniswap_v3::UniswapV3Pool, IUniswapV2Pair,
};
use crate::flight_recorder::FlightRecorder;
use crate::{record_phase_start, record_phase_end, record_decision, record_rpc_call, record_error};
use crate::multicall::{Call, Multicall};
use crate::pool_blacklist::GLOBAL_CORRUPTION_TRACKER; // NOTE: Renamed from pool_corruption_tracker
use crate::rpc_pool::RpcPool;
use crate::settings::Settings;
use crate::v3_math::V3PoolState;
use std::sync::Arc as StdArc;
use dashmap::DashMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use crate::cache_state::{StateCacheManager, CacheValidationResult};
use crate::cache_state::state_cache::CachedPoolState as StateCacheCachedPoolState;

/// Pool metadata needed for JIT state fetching.
///
/// Contains the essential information required to fetch pool state on-demand.
#[derive(Debug, Clone)]
pub struct PoolMetadata {
    /// Pool contract address
    pub address: Address,
    /// Pool type (V2 or V3)
    pub pool_type: PoolType,
    /// First token in the pair
    pub token0: Address,
    /// Second token in the pair
    pub token1: Address,
    /// Fee tier for V3 pools (in basis points, e.g., 3000 for 0.3%)
    pub fee: Option<u32>,
}

/// Pool type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType {
    /// Uniswap V2-style pool (constant product formula)
    V2,
    /// Uniswap V3-style pool (concentrated liquidity)
    V3,
}

/// Fresh pool states fetched via JIT multicall.
///
/// Contains the latest state for pools fetched in a single batch operation.
#[derive(Debug, Clone)]
pub struct FreshPoolStates {
    /// V3 pool states keyed by pool address
    pub v3_states: HashMap<Address, V3PoolState>,
    /// V2 pool reserves keyed by pool address (reserve0, reserve1)
    pub v2_states: HashMap<Address, (U256, U256)>,
    /// V2 pool token metadata needed to orient reserve0/reserve1 correctly
    pub v2_tokens: HashMap<Address, (Address, Address)>,
    /// Total time taken to fetch all states
    pub fetch_duration: std::time::Duration,
    /// Total number of RPC calls made
    pub total_calls: usize,
    /// Number of successful RPC calls
    pub successful_calls: usize,
    /// Block number at which states were fetched
    pub block_number: u64,
}

// ✅ P0 OPTIMIZATION: Using CachedPoolState from cache_state module instead of local duplicate
// The local CachedPoolState struct has been removed and we now use cache_state::state_cache::CachedPoolState

/// Just-In-Time state fetcher for on-demand pool state synchronization.
///
/// The JIT State Fetcher implements an optimized strategy for fetching pool states
/// only when needed, with aggressive caching that invalidates only when pool state changes.
///
/// ## Features
///
/// - **On-Demand Fetching**: Fetches pool states only when requested
/// - **Fuzzy Block Matching**: Cache tolerance for blocks within configurable range
/// - **Multicall Batching**: Batches multiple pool state queries into single RPC calls
/// - **State Hash Validation**: Invalidates cache only when pool state actually changes
///
/// ## Performance
///
/// - Cache hit rate target: >80%
/// - Latency: <100ms for cache miss (local node: <10ms)
/// - RPC call reduction: >80% via caching
///
/// See `docs/ARCHITECTURE.md` for detailed architecture information.
pub struct JitStateFetcher {
    rpc_pool: Arc<RpcPool>,
    multicall_address: Address,
    batch_size: usize,
    settings: StdArc<Settings>,
    // 🚀 RPC OPTIMIZATION: Cache states by pool address, invalidate only when state hash changes
    state_cache: Arc<DashMap<Address, StateCacheCachedPoolState>>,
    // ✅ P0 OPTIMIZATION: State cache manager for Merkle tree-based invalidation
    state_cache_manager: StateCacheManager,
    // ✅ P0 OPTIMIZATION: Hot Pool Manager for top-K pool caching
    hot_pool_manager: Option<Arc<crate::hot_pool_manager::HotPoolManager>>,
    // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    flight_recorder: Option<Arc<FlightRecorder>>,
    // ✅ REDIS: Optional Redis backend for distributed caching
    #[cfg(feature = "redis")]
    redis_manager: Option<Arc<tokio::sync::Mutex<crate::redis_manager::RedisManager>>>,
}

impl JitStateFetcher {
    pub fn new(
        rpc_pool: Arc<RpcPool>,
        multicall_address: Address,
        batch_size: usize,
        settings: StdArc<Settings>,
    ) -> Self {
        Self {
            rpc_pool,
            multicall_address,
            batch_size,
            settings,
            state_cache: Arc::new(DashMap::new()),
            state_cache_manager: StateCacheManager::new(2000), // Max 2000 cached pools
            flight_recorder: None,
            #[cfg(feature = "redis")]
            redis_manager: None,
            hot_pool_manager: None,
        }
    }
    
    /// ✅ P0 OPTIMIZATION: Set Hot Pool Manager for weight lookups and cache pre-warming
    pub fn with_hot_pool_manager(mut self, hot_pool_manager: Arc<crate::hot_pool_manager::HotPoolManager>) -> Self {
        self.hot_pool_manager = Some(hot_pool_manager);
        self
    }
    
    /// ✅ P0 OPTIMIZATION: Get pool weight from Hot Pool Manager or return None
    fn get_pool_weight(&self, pool_address: &Address) -> Option<f64> {
        self.hot_pool_manager.as_ref()
            .and_then(|hpm| hpm.get_pool_weight(pool_address))
    }
    
    /// ✅ P0 OPTIMIZATION: Fallback to Hot Pool Manager for cache misses
    /// If a pool is not in cache and Hot Pool Manager has it, use that state
    fn get_pool_state_from_hot_pool_manager(&self, pool_address: &Address) -> Option<StateCacheCachedPoolState> {
        let hot_pool_manager = self.hot_pool_manager.as_ref()?;
        
        // Try V3 pools first
        if let Some(snapshot) = hot_pool_manager.v3_hot_pools.get(pool_address) {
            let v3_state = V3PoolState {
                sqrt_price_x96: snapshot.state.sqrt_price_x96,
                tick: snapshot.state.tick,
                liquidity: snapshot.state.liquidity,
            };
            
            let merkle_root = Self::calculate_merkle_root(Some(&v3_state), None, 0);
            
            return Some(StateCacheCachedPoolState {
                v3_state: Some(v3_state),
                v2_state: None,
                v2_token0: Some(snapshot.token0),
                v2_token1: Some(snapshot.token1),
                merkle_root,
                block_number: 0, // Will be updated on first fetch
                last_updated: snapshot.last_updated,
                touched: false,
            });
        }
        
        // Try V2 pools
        if let Some(snapshot) = hot_pool_manager.v2_hot_pools.get(pool_address) {
            let v2_state = (snapshot.reserve0, snapshot.reserve1);
            
            let merkle_root = Self::calculate_merkle_root(None, Some(&v2_state), 0);
            
            return Some(StateCacheCachedPoolState {
                v3_state: None,
                v2_state: Some(v2_state),
                v2_token0: Some(snapshot.token0),
                v2_token1: Some(snapshot.token1),
                merkle_root,
                block_number: 0, // Will be updated on first fetch
                last_updated: snapshot.last_updated,
                touched: false,
            });
        }
        
        None
    }
    
    /// ✅ P0 OPTIMIZATION: Pre-warm cache with top-K pools from Hot Pool Manager
    /// This function loads pool states from Hot Pool Manager into the JIT cache
    /// to improve initial cache hit rate
    pub async fn pre_warm_cache_from_hot_pool_manager(&self, limit: usize) -> Result<usize> {
        let hot_pool_manager = match &self.hot_pool_manager {
            Some(hpm) => hpm,
            None => {
                debug!("⚠️ [JIT Cache Pre-warm] Hot Pool Manager not available, skipping pre-warm");
                return Ok(0);
            }
        };
        
        let start = Instant::now();
        let mut pre_warmed_count = 0;
        
        // Get top-K pools from Hot Pool Manager
        // We'll iterate through V3 and V2 pools
        let v3_pools: Vec<_> = hot_pool_manager.v3_hot_pools.iter()
            .take(limit)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        
        let v2_pools: Vec<_> = hot_pool_manager.v2_hot_pools.iter()
            .take(limit.saturating_sub(v3_pools.len()))
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        
        // Pre-warm V3 pools
        for (pool_addr, snapshot) in v3_pools {
            let v3_state = V3PoolState {
                sqrt_price_x96: snapshot.state.sqrt_price_x96,
                tick: snapshot.state.tick,
                liquidity: snapshot.state.liquidity,
            };
            
            let merkle_root = Self::calculate_merkle_root(Some(&v3_state), None, 0);
            
            self.state_cache.insert(pool_addr, StateCacheCachedPoolState {
                v3_state: Some(v3_state),
                v2_state: None,
                v2_token0: Some(snapshot.token0),
                v2_token1: Some(snapshot.token1),
                merkle_root,
                block_number: 0, // Will be updated on first fetch
                last_updated: snapshot.last_updated,
                touched: false,
            });
            pre_warmed_count += 1;
        }
        
        // Pre-warm V2 pools
        for (pool_addr, snapshot) in v2_pools {
            let v2_state = (snapshot.reserve0, snapshot.reserve1);
            
            let merkle_root = Self::calculate_merkle_root(None, Some(&v2_state), 0);
            
            self.state_cache.insert(pool_addr, StateCacheCachedPoolState {
                v3_state: None,
                v2_state: Some(v2_state),
                v2_token0: Some(snapshot.token0),
                v2_token1: Some(snapshot.token1),
                merkle_root,
                block_number: 0, // Will be updated on first fetch
                last_updated: snapshot.last_updated,
                touched: false,
            });
            pre_warmed_count += 1;
        }
        
        info!("✅ [JIT Cache Pre-warm] Pre-warmed {} pools from Hot Pool Manager in {:?}", 
              pre_warmed_count, start.elapsed());
        
        Ok(pre_warmed_count)
    }
    
    /// Set flight recorder for instrumentation
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder);
        self
    }
    
    /// Set Redis manager for distributed caching (optional, requires redis feature)
    #[cfg(feature = "redis")]
    pub fn with_redis(mut self, redis_manager: Arc<tokio::sync::Mutex<crate::redis_manager::RedisManager>>) -> Self {
        self.redis_manager = Some(redis_manager);
        self
    }
    
    /// ✅ FASE 2.2: Calculate Merkle tree hash for pool state
    /// Hash combines block_number and state_hash for stronger invalidation guarantees
    fn calculate_merkle_root(
        v3_state: Option<&V3PoolState>,
        v2_state: Option<&(U256, U256)>,
        block_number: u64,
    ) -> [u8; 32] {
        // First, calculate state hash
        let mut state_hasher = DefaultHasher::new();
        if let Some(v3) = v3_state {
            v3.sqrt_price_x96.hash(&mut state_hasher);
            v3.liquidity.hash(&mut state_hasher);
            v3.tick.hash(&mut state_hasher);
        }
        if let Some(v2) = v2_state {
            v2.0.hash(&mut state_hasher);
            v2.1.hash(&mut state_hasher);
        }
        let state_hash = state_hasher.finish();
        
        // Combine block_number and state_hash into Merkle root using keccak256
        use ethers::utils::keccak256;
        let mut combined = Vec::with_capacity(16);
        combined.extend_from_slice(&block_number.to_be_bytes());
        combined.extend_from_slice(&state_hash.to_be_bytes());
        keccak256(combined)
    }
    
    /// ✅ FASE 2.2: Update Merkle root incrementally (for streaming results)
    fn update_merkle_root_incremental(
        current_root: Option<[u8; 32]>,
        new_state: &(Option<&V3PoolState>, Option<&(U256, U256)>),
        block_number: u64,
    ) -> [u8; 32] {
        Self::calculate_merkle_root(new_state.0, new_state.1, block_number)
    }

    /// Fetch current states for pools needed by routes (JIT Dynamic Refresh)
    /// 🚀 RPC OPTIMIZATION: Uses aggressive caching - only fetches if pool state changed
    /// This is the core function: takes pool addresses + metadata, returns fresh states
    /// 
    /// ✅ FASE 2.2: `touched_pools` opcional - si se proporciona, pools no-touched priorizan cache
    pub async fn fetch_current_states(
        &self,
        pool_metadata: &[PoolMetadata],
        current_block: u64,
    ) -> Result<FreshPoolStates> {
        self.fetch_current_states_with_touched(pool_metadata, current_block, None).await
    }
    
    /// Fetch current states with optional touched_pools set for cache optimization
    /// ✅ FASE 2.2: Pools no-touched priorizan cache antes de fetch
    pub async fn fetch_current_states_with_touched(
        &self,
        pool_metadata: &[PoolMetadata],
        current_block: u64,
        touched_pools: Option<&HashSet<Address>>,
    ) -> Result<FreshPoolStates> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de JIT fetch
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "jit_fetch_internal", serde_json::json!({
                "total_pools": pool_metadata.len(),
                "touched_pools": touched_pools.map(|tp| tp.len()).unwrap_or(0),
                "current_block": current_block
            }));
        }
        
        // 🚀 RPC OPTIMIZATION: Check cache first - only fetch pools that changed
        let mut pools_to_fetch = Vec::new();
        let mut cached_v3_states: HashMap<Address, V3PoolState> = HashMap::new();
        let mut cached_v2_states: HashMap<Address, (U256, U256)> = HashMap::new();
        let mut cached_v2_tokens: HashMap<Address, (Address, Address)> = HashMap::new();
        let mut v2_token_only_pools: Vec<Address> = Vec::new();
        
        // ✅ DEBUG: Verificar tamaño del cache antes de verificar
        let _cache_size_before = self.state_cache.len();
        
        // ✅ OPTIMIZACIÓN CACHE: Usar cache más agresivamente incluso para pools touched
        // Estrategia: Si el cache es del mismo bloque o bloque anterior, usarlo (estado no cambió)
        // ✅ SPRINT ESTABILIZACIÓN: Fuzzy Block Matching con configuración desde Settings
        let cache_tolerance_blocks = self.settings.performance.jit_cache_tolerance_blocks;
        let cache_ttl_ms = self.settings.performance.jit_cache_ttl_ms;
        let cache_ttl = std::time::Duration::from_millis(cache_ttl_ms);
        
        for meta in pool_metadata {
            let is_touched = touched_pools.map(|tp| tp.contains(&meta.address)).unwrap_or(true);
            
            // ✅ SPRINT ESTABILIZACIÓN: Verificar cache primero para TODOS los pools
            // Incluso pools "touched" pueden usar cache si es del mismo bloque
            let mut use_cache = false;
            let mut _cache_invalidation_reason: Option<String> = None;
            
            if let Some(cached) = self.state_cache.get(&meta.address) {
                let cached_state: &StateCacheCachedPoolState = cached.value();
                
                // ✅ P0 OPTIMIZATION: Fuzzy Block Matching mejorado con StateCacheManager
                // Cache válido si: (current_block - cached_block <= tolerance) AND (elapsed_time < TTL)
                let elapsed_time = cached_state.last_updated.elapsed();
                let blocks_since_cache = current_block.saturating_sub(cached_state.block_number);
                
                // ✅ P0 OPTIMIZATION: TTL diferenciado completo
                // Para pools touched: tolerancia más estricta (1 bloque, TTL configurable)
                // Para pools no-touched: tolerancia más permisiva (configurable, TTL configurable)
                let block_tolerance = if is_touched { 1 } else { cache_tolerance_blocks };
                
                // ✅ P0 OPTIMIZATION: TTL diferenciado completo con adaptive TTL
                let time_tolerance = if is_touched {
                    std::time::Duration::from_secs(self.settings.performance.touched_pool_ttl_seconds)
                } else {
                    // ✅ P0 OPTIMIZATION: Adaptive TTL basado en pool weight
                    let pool_weight = self.get_pool_weight(&meta.address).unwrap_or(0.0);
                    if self.settings.performance.adaptive_ttl_enabled && 
                       pool_weight > self.settings.performance.adaptive_ttl_weight_threshold {
                        // High-weight pools: shorter TTL (half of untouched TTL)
                        std::time::Duration::from_secs(self.settings.performance.untouched_pool_ttl_seconds / 2)
                    } else {
                        // Normal untouched pools: full TTL
                        std::time::Duration::from_secs(self.settings.performance.untouched_pool_ttl_seconds)
                    }
                };
                
                // ✅ P0 OPTIMIZATION: Usar StateCacheManager para validación híbrida
                let validation_result = StateCacheManager::validate_cache_hybrid(
                    cached_state,
                    current_block,
                    block_tolerance,
                    time_tolerance,
                );
                
                let cache_valid = validation_result == CacheValidationResult::Valid;
                
                // ✅ P0 OPTIMIZATION: Logs detallados usando validation_result
                if !cache_valid {
                    let reason = match validation_result {
                        CacheValidationResult::InvalidStateHash => {
                            format!("State hash mismatch")
                        }
                        CacheValidationResult::InvalidBlockTolerance => {
                            format!("Block Diff {} > Tolerance {}", blocks_since_cache, block_tolerance)
                        }
                        CacheValidationResult::InvalidTTL => {
                            format!("Elapsed {}ms > TTL {}ms", elapsed_time.as_millis(), time_tolerance.as_millis())
                        }
                        CacheValidationResult::NotFound => {
                            format!("Cache entry not found")
                        }
                        CacheValidationResult::Valid => {
                            format!("Cache valid")
                        }
                    };
                    _cache_invalidation_reason = Some(reason.clone());
                    debug!("🔍 [JIT Cache] Pool {:?} cache invalid: {} (current_block={}, cached_block={}, is_touched={})", 
                           meta.address, reason, current_block, cached_state.block_number, is_touched);
                } else {
                    debug!("✅ [JIT Cache] Pool {:?} cache VALID: blocks_since={} <= tolerance={}, elapsed={}ms < ttl={}ms", 
                           meta.address, blocks_since_cache, block_tolerance, elapsed_time.as_millis(), time_tolerance.as_millis());
                }
                
                // ✅ P0 OPTIMIZATION: Si el cache es válido según StateCacheManager, usarlo
                if cache_valid {
                    // If pool became corrupted, don't use cached state/tokens for it.
                    if meta.pool_type == PoolType::V2 && GLOBAL_CORRUPTION_TRACKER.is_corrupted(meta.address) {
                        _cache_invalidation_reason = Some("Pool marked as corrupted".to_string());
                        crate::metrics::increment_counter_named("jit_skipped_corrupted_v2_pool_total".to_string());
                        continue;
                    }
                    // Use cached state
                    if let Some(ref v3) = cached_state.v3_state {
                        cached_v3_states.insert(meta.address, v3.clone());
                    }
                    if let Some(ref v2) = cached_state.v2_state {
                        cached_v2_states.insert(meta.address, *v2);
                    }
                    // Cache V2 token0/token1 if available; if missing, schedule token-only fetch.
                    if meta.pool_type == PoolType::V2 {
                        if let (Some(t0), Some(t1)) = (cached_state.v2_token0, cached_state.v2_token1) {
                            cached_v2_tokens.insert(meta.address, (t0, t1));
                        } else {
                            v2_token_only_pools.push(meta.address);
                        }
                    }
                    // Only skip fetching if we actually have the required state cached.
                    match meta.pool_type {
                        PoolType::V3 if cached_state.v3_state.is_some() => {
                            use_cache = true; // Usar cache, no necesita fetch
                        }
                        PoolType::V2 if cached_state.v2_state.is_some() => {
                            use_cache = true; // Usar cache, no necesita fetch
                        }
                        _ => {
                            // Missing required state, fall through to fetch.
                            _cache_invalidation_reason = Some("Missing required state in cache".to_string());
                        }
                    }
                }
            } else {
                // ✅ P0 OPTIMIZATION: Fallback to Hot Pool Manager for cache misses
                if let Some(hpm_state) = self.get_pool_state_from_hot_pool_manager(&meta.address) {
                    debug!("✅ [JIT Cache] Pool {:?} found in Hot Pool Manager, using as fallback", meta.address);
                    
                    // Use Hot Pool Manager state as cached state
                    if let Some(ref v3) = hpm_state.v3_state {
                        cached_v3_states.insert(meta.address, v3.clone());
                    }
                    if let Some(ref v2) = hpm_state.v2_state {
                        cached_v2_states.insert(meta.address, *v2);
                    }
                    if meta.pool_type == PoolType::V2 {
                        if let (Some(t0), Some(t1)) = (hpm_state.v2_token0, hpm_state.v2_token1) {
                            cached_v2_tokens.insert(meta.address, (t0, t1));
                        }
                    }
                    
                    // Add to cache for next time
                    self.state_cache.insert(meta.address, hpm_state);
                    use_cache = true;
                } else {
                    _cache_invalidation_reason = Some("Pool not found in cache or Hot Pool Manager".to_string());
                    debug!("🔍 [JIT Cache] Pool {:?} not in cache or Hot Pool Manager (cache_size={})", meta.address, self.state_cache.len());
                }
            }
            
            if !use_cache {
                // Pool tocado o cache miss/stale: necesita fetch on-chain
                if let Some(ref reason) = _cache_invalidation_reason {
                    debug!("🔄 [JIT Cache] Pool {:?} needs fetch: {}", meta.address, reason);
                }
                pools_to_fetch.push(meta.clone());
            }
        }
        
        let cached_count = cached_v3_states.len() + cached_v2_states.len();
        let cache_hit_rate = if pool_metadata.is_empty() {
            0.0
        } else {
            (cached_count as f64 / pool_metadata.len() as f64) * 100.0
        };
        
        info!("🔄 [JIT] Cache hit: {} pools cached ({} V3, {} V2), {} pools need fetch (cache_size={}, current_block={}, hit_rate={:.1}%)", 
              cached_count, cached_v3_states.len(), cached_v2_states.len(), pools_to_fetch.len(), 
              self.state_cache.len(), current_block, cache_hit_rate);
        
        // ✅ OPTIMIZACIÓN CU/s: Skip fetch si cache hit rate > 90%
        // Si la mayoría de pools vienen del cache, usar HotPoolManager como fallback
        if cache_hit_rate > 90.0 && pools_to_fetch.len() > 0 {
            info!("✅ [JIT] Cache hit rate {:.1}% > 90%, skipping fetch for {} pools (using HotPoolManager fallback)", 
                  cache_hit_rate, pools_to_fetch.len());
            // Retornar solo cache, HotPoolManager se usará como fallback en mvp_runner
            return Ok(FreshPoolStates {
                v3_states: cached_v3_states,
                v2_states: cached_v2_states,
                v2_tokens: cached_v2_tokens,
                fetch_duration: start_time.elapsed(),
                total_calls: 0,
                successful_calls: 0,
                block_number: current_block,
            });
        }
        
        // ✅ OPTIMIZACIÓN LATENCIA + CU/s: Fast path para 1-3 pools
        // Detectar casos edge y usar RPC directo (más rápido y eficiente que multicall)
        // Esto resuelve el problema de que multicall falla 95% cuando hay 1 pool
        if pools_to_fetch.len() <= 3 {
            info!("🚀 [JIT Fast Path] {} pools to fetch (using fast path for edge cases)", pools_to_fetch.len());
            
            // Para múltiples pools (2-3), fetchear secuencialmente usando fast path
            // (más simple que paralelo y suficiente para casos edge)
            if pools_to_fetch.len() > 1 {
                let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
                let mut fast_path_results = FreshPoolStates {
                    v3_states: cached_v3_states.clone(),
                    v2_states: cached_v2_states.clone(),
                    v2_tokens: cached_v2_tokens.clone(),
                    fetch_duration: start_time.elapsed(),
                    total_calls: 0,
                    successful_calls: 0,
                    block_number: current_block,
                };
                
                // Fetch pools secuencialmente (suficiente para 2-3 pools)
                for pool in &pools_to_fetch {
                    match self.fetch_single_pool_direct(pool, Arc::clone(&provider), Some(current_block)).await {
                        Ok(Some(fresh)) => {
                            fast_path_results.v3_states.extend(fresh.v3_states);
                            fast_path_results.v2_states.extend(fresh.v2_states);
                            fast_path_results.v2_tokens.extend(fresh.v2_tokens);
                            fast_path_results.total_calls += fresh.total_calls;
                            fast_path_results.successful_calls += fresh.successful_calls;
                        }
                        _ => {
                            // Pool inválido o error - continuar con siguiente
                            debug!("⚠️ [JIT Fast Path] Pool {:?} fetch failed, skipping", pool.address);
                        }
                    }
                }
                
                // Update cache
                for (addr, state) in &fast_path_results.v3_states {
                    if !cached_v3_states.contains_key(addr) {
                        let merkle_root = Self::calculate_merkle_root(Some(state), None, current_block);
                        self.state_cache.insert(*addr, StateCacheCachedPoolState {
                            v3_state: Some(state.clone()),
                            v2_state: None,
                            v2_token0: None,
                            v2_token1: None,
                            merkle_root,
                            block_number: current_block,
                            last_updated: Instant::now(),
                            touched: false,
                        });
                    }
                }
                for (addr, reserves) in &fast_path_results.v2_states {
                    if !cached_v2_states.contains_key(addr) {
                        let (token0, token1) = fast_path_results.v2_tokens.get(addr).copied().unwrap_or((Address::zero(), Address::zero()));
                        let merkle_root = Self::calculate_merkle_root(None, Some(reserves), current_block);
                        self.state_cache.insert(*addr, StateCacheCachedPoolState {
                            v3_state: None,
                            v2_state: Some(*reserves),
                            v2_token0: Some(token0),
                            v2_token1: Some(token1),
                            merkle_root,
                            block_number: current_block,
                            last_updated: Instant::now(),
                            touched: false,
                        });
                    }
                }
                
                fast_path_results.fetch_duration = start_time.elapsed();
                
                if let Some(ref recorder) = self.flight_recorder {
                    record_phase_end!(recorder, "jit_fetch_internal", start_time, serde_json::json!({
                        "cached_count": cached_count,
                        "cache_hits": cached_count,
                        "cache_misses": pools_to_fetch.len(),
                        "total_pools": pool_metadata.len(),
                        "pools_to_fetch": pools_to_fetch.len(),
                        "total_calls": fast_path_results.total_calls,
                        "successful_calls": fast_path_results.successful_calls,
                        "cache_hit_rate": if pool_metadata.is_empty() { 0.0 } else {
                            (cached_count as f64 / pool_metadata.len() as f64) * 100.0
                        },
                        "fast_path": true,
                        "fast_path_pools": pools_to_fetch.len(),
                    }), current_block);
                }
                
                info!("✅ [JIT Fast Path] {} pools fetched in {}ms ({} V3, {} V2 states)",
                      pools_to_fetch.len(), fast_path_results.fetch_duration.as_millis(),
                      fast_path_results.v3_states.len(), fast_path_results.v2_states.len());
                
                return Ok(fast_path_results);
            }
            
            // Single pool case (original logic)
            let pool = &pools_to_fetch[0];
            info!("🚀 [JIT Fast Path] Single pool to fetch: {:?} (type: {:?})", pool.address, pool.pool_type);
            
            // Record inicio de fast path
            if let Some(ref recorder) = self.flight_recorder {
                record_phase_start!(recorder, "jit_fast_path_single", serde_json::json!({
                    "pool": format!("{:?}", pool.address),
                    "pool_type": format!("{:?}", pool.pool_type),
                }), current_block);
            }
            
            let fast_path_start = Instant::now();
            
            // Get RPC provider
            let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
            
            // Fetch directo usando llamadas RPC individuales
            match self.fetch_single_pool_direct(pool, Arc::clone(&provider), Some(current_block)).await {
                Ok(Some(fresh_states)) => {
                    // Merge con cache
                    let mut result = FreshPoolStates {
                        v3_states: cached_v3_states.clone(),
                        v2_states: cached_v2_states.clone(),
                        v2_tokens: cached_v2_tokens.clone(),
                        fetch_duration: start_time.elapsed(),
                        total_calls: fresh_states.total_calls,
                        successful_calls: fresh_states.successful_calls,
                        block_number: current_block,
                    };
                    
                    result.v3_states.extend(fresh_states.v3_states);
                    result.v2_states.extend(fresh_states.v2_states);
                    result.v2_tokens.extend(fresh_states.v2_tokens);
                    
                    // ✅ CORRECCIÓN: Update cache con estructura real de CachedPoolState
                    for (addr, state) in &result.v3_states {
                        if !cached_v3_states.contains_key(addr) {
                            let merkle_root = Self::calculate_merkle_root(Some(state), None, current_block);
                            self.state_cache.insert(*addr, StateCacheCachedPoolState {
                                v3_state: Some(state.clone()),
                                v2_state: None,
                                v2_token0: None,
                                v2_token1: None,
                                merkle_root,
                                block_number: current_block,
                                last_updated: Instant::now(),
                                touched: false,
                            });
                        }
                    }
                    for (addr, reserves) in &result.v2_states {
                        if !cached_v2_states.contains_key(addr) {
                            let (token0, token1) = result.v2_tokens.get(addr).copied().unwrap_or((Address::zero(), Address::zero()));
                            let merkle_root = Self::calculate_merkle_root(None, Some(reserves), current_block);
                            self.state_cache.insert(*addr, StateCacheCachedPoolState {
                                v3_state: None,
                                v2_state: Some(*reserves),
                                v2_token0: Some(token0),
                                v2_token1: Some(token1),
                                merkle_root,
                                block_number: current_block,
                                last_updated: Instant::now(),
                                touched: false,
                            });
                        }
                    }
                    
                    // Record fin exitoso
                    if let Some(ref recorder) = self.flight_recorder {
                        record_phase_end!(recorder, "jit_fast_path_single", fast_path_start, serde_json::json!({
                            "success": true,
                            "v3_states": result.v3_states.len(),
                            "v2_states": result.v2_states.len(),
                            "duration_ms": fast_path_start.elapsed().as_millis(),
                        }), current_block);
                    }
                    
                    info!("✅ [JIT Fast Path] Single pool fetch succeeded in {}ms ({} V3, {} V2 states)",
                          fast_path_start.elapsed().as_millis(), result.v3_states.len(), result.v2_states.len());
                    
                    // Record fin de jit_fetch_internal
                    if let Some(ref recorder) = self.flight_recorder {
                        record_phase_end!(recorder, "jit_fetch_internal", start_time, serde_json::json!({
                            "cached_count": cached_count,
                            "cache_hits": cached_count,
                            "cache_misses": 1,
                            "total_pools": pool_metadata.len(),
                            "pools_to_fetch": 1,
                            "total_calls": fresh_states.total_calls,
                            "successful_calls": fresh_states.successful_calls,
                            "cache_hit_rate": if pool_metadata.is_empty() { 0.0 } else {
                                (cached_count as f64 / pool_metadata.len() as f64) * 100.0
                            },
                            "fast_path": true,
                        }), current_block);
                    }
                    
                    return Ok(result);
                }
                Ok(None) => {
                    // Pool no existe o está inválido - OK, continuar con cache
                    debug!("⚠️ [JIT Fast Path] Single pool {:?} invalid or doesn't exist (expected for some pools)", pool.address);
                    
                    // Record fin sin pool válido
                    if let Some(ref recorder) = self.flight_recorder {
                        record_phase_end!(recorder, "jit_fast_path_single", fast_path_start, serde_json::json!({
                            "success": false,
                            "reason": "pool_invalid_or_nonexistent",
                            "duration_ms": fast_path_start.elapsed().as_millis(),
                        }), current_block);
                    }
                    
                    // Retornar solo cache (sin el pool que no existe)
                    if let Some(ref recorder) = self.flight_recorder {
                        record_phase_end!(recorder, "jit_fetch_internal", start_time, serde_json::json!({
                            "cached_count": cached_count,
                            "cache_hits": cached_count,
                            "cache_misses": 1,
                            "total_pools": pool_metadata.len(),
                            "pools_to_fetch": 1,
                            "total_calls": 2,
                            "successful_calls": 0,
                            "cache_hit_rate": if pool_metadata.is_empty() { 0.0 } else {
                                (cached_count as f64 / pool_metadata.len() as f64) * 100.0
                            },
                            "fast_path": true,
                            "pool_fetch_failed": true,
                        }), current_block);
                    }
                    
                    return Ok(FreshPoolStates {
                        v3_states: cached_v3_states,
                        v2_states: cached_v2_states,
                        v2_tokens: cached_v2_tokens,
                        fetch_duration: start_time.elapsed(),
                        total_calls: 2,
                        successful_calls: 0,
                        block_number: current_block,
                    });
                }
                Err(e) => {
                    warn!("⚠️ [JIT Fast Path] Error fetching single pool {:?}: {}. Falling back to cache only.", pool.address, e);
                    
                    // Record error
                    if let Some(ref recorder) = self.flight_recorder {
                        record_error!(recorder, "jit_state_fetcher", "fast_path_error", &format!("{}", e), serde_json::json!({
                            "pool": format!("{:?}", pool.address),
                            "pool_type": format!("{:?}", pool.pool_type),
                        }), current_block);
                    }
                    
                    // Retornar solo cache
                    if let Some(ref recorder) = self.flight_recorder {
                        record_phase_end!(recorder, "jit_fetch_internal", start_time, serde_json::json!({
                            "cached_count": cached_count,
                            "cache_hits": cached_count,
                            "cache_misses": 1,
                            "total_pools": pool_metadata.len(),
                            "pools_to_fetch": 1,
                            "total_calls": 2,
                            "successful_calls": 0,
                            "cache_hit_rate": if pool_metadata.is_empty() { 0.0 } else {
                                (cached_count as f64 / pool_metadata.len() as f64) * 100.0
                            },
                            "fast_path": true,
                            "error": true,
                        }), current_block);
                    }
                    
                    return Ok(FreshPoolStates {
                        v3_states: cached_v3_states,
                        v2_states: cached_v2_states,
                        v2_tokens: cached_v2_tokens,
                        fetch_duration: start_time.elapsed(),
                        total_calls: 2,
                        successful_calls: 0,
                        block_number: current_block,
                    });
                }
            }
        }
        
        // ✅ P0 OPTIMIZATION: Separate pools by touched/untouched for batch prioritization
        let mut touched_to_fetch = Vec::new();
        let mut untouched_to_fetch = Vec::new();
        
        for meta in &pools_to_fetch {
            let is_touched = touched_pools.map(|tp| tp.contains(&meta.address)).unwrap_or(false);
            if is_touched {
                touched_to_fetch.push(meta.clone());
            } else {
                untouched_to_fetch.push(meta.clone());
            }
        }
        
        // Separate pools to fetch by type (for touched and untouched separately)
        let mut touched_v3_pools = Vec::new();
        let mut touched_v2_pools = Vec::new();
        let mut untouched_v3_pools = Vec::new();
        let mut untouched_v2_pools = Vec::new();
        
        for meta in &touched_to_fetch {
            match meta.pool_type {
                PoolType::V3 => touched_v3_pools.push(meta.clone()),
                PoolType::V2 => touched_v2_pools.push(meta.clone()),
            }
        }
        
        for meta in &untouched_to_fetch {
            match meta.pool_type {
                PoolType::V3 => untouched_v3_pools.push(meta.clone()),
                PoolType::V2 => untouched_v2_pools.push(meta.clone()),
            }
        }
        
        let total_v3 = touched_v3_pools.len() + untouched_v3_pools.len();
        let total_v2 = touched_v2_pools.len() + untouched_v2_pools.len();
        
        info!(
            "🔄 [JIT] Fetching states for {} pools ({} V3, {} V2) at block {} ({} touched, {} untouched)",
            pools_to_fetch.len(),
            total_v3,
            total_v2,
            current_block,
            touched_to_fetch.len(),
            untouched_to_fetch.len()
        );
        
        // Get next RPC provider
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        
        // ✅ OPTIMIZACIÓN JIT: Reutilizar contratos dummy para reducir overhead de construcción
        // Crear contratos dummy una vez y reutilizar calldata (solo cambia el target address)
        let dummy_v3_contract = UniswapV3Pool::new(Address::zero(), Arc::clone(&provider));
        let dummy_v2_contract = IUniswapV2Pair::new(Address::zero(), Arc::clone(&provider));
        
        // Pre-calcular calldata para funciones comunes (solo una vez)
        let v3_slot0_calldata = dummy_v3_contract.slot_0().calldata().unwrap();
        let v3_liquidity_calldata = dummy_v3_contract.liquidity().calldata().unwrap();
        let v2_reserves_calldata = dummy_v2_contract.get_reserves().calldata().unwrap();
        let v2_token0_calldata = dummy_v2_contract.token_0().calldata().unwrap();
        let v2_token1_calldata = dummy_v2_contract.token_1().calldata().unwrap();
        
        // ✅ P0 OPTIMIZATION: Build multicall requests with prioritization (touched first, then untouched)
        // Helper function to build calls for a set of pools
        let mut build_calls_for_pools = |v3_pools: &[PoolMetadata], v2_pools: &[PoolMetadata]| -> (Vec<Call>, Vec<(&str, Address, PoolType)>) {
            let mut calls = Vec::new();
            let mut call_index_map = Vec::new();
            
            // V3 pools: slot0 + liquidity
            for meta in v3_pools {
                calls.push(Call {
                    target: meta.address,
                    call_data: v3_slot0_calldata.clone(),
                });
                call_index_map.push(("v3_slot0", meta.address, PoolType::V3));
                
                calls.push(Call {
                    target: meta.address,
                    call_data: v3_liquidity_calldata.clone(),
                });
                call_index_map.push(("v3_liquidity", meta.address, PoolType::V3));
            }
            
            // V2 pools: getReserves
            for meta in v2_pools {
                calls.push(Call {
                    target: meta.address,
                    call_data: v2_reserves_calldata.clone(),
                });
                call_index_map.push(("v2_reserves", meta.address, PoolType::V2));

                // Fetch token0/token1 only if we don't have them cached yet.
                if !cached_v2_tokens.contains_key(&meta.address) {
                    calls.push(Call { target: meta.address, call_data: v2_token0_calldata.clone() });
                    call_index_map.push(("v2_token0", meta.address, PoolType::V2));
                    calls.push(Call { target: meta.address, call_data: v2_token1_calldata.clone() });
                    call_index_map.push(("v2_token1", meta.address, PoolType::V2));
                }
            }
            
            (calls, call_index_map)
        };
        
        // ✅ P0 OPTIMIZATION: Build calls for touched pools first (high priority, small batches)
        let (mut touched_calls, mut touched_call_index_map) = build_calls_for_pools(&touched_v3_pools, &touched_v2_pools);
        
        // ✅ P0 OPTIMIZATION: Build calls for untouched pools (low priority, can be larger batches)
        let (untouched_calls, untouched_call_index_map) = build_calls_for_pools(&untouched_v3_pools, &untouched_v2_pools);
        
        // ✅ P0 OPTIMIZATION: Save counts and calculate batch sizes BEFORE moving values
        let touched_count = touched_calls.len();
        let untouched_count = untouched_calls.len();
        
        let touched_batch_size = if touched_count <= 20 {
            // Small batches for touched pools (10-20 calls) for faster response
            touched_count.max(10).min(20)
        } else {
            20 // Cap at 20 for touched pools
        };
        
        let untouched_batch_size = if untouched_count <= 200 {
            // Larger batches for untouched pools (up to 200 calls)
            untouched_count.max(50).min(200)
        } else {
            200 // Cap at 200 for untouched pools
        };
        
        // Combine: touched first, then untouched
        let mut calls = touched_calls;
        let mut call_index_map = touched_call_index_map;
        calls.extend(untouched_calls);
        call_index_map.extend(untouched_call_index_map);

        // V2 token-only pools (state cached but missing token0/token1): fetch token0/token1 without reserves.
        for pool_addr in &v2_token_only_pools {
            calls.push(Call { target: *pool_addr, call_data: v2_token0_calldata.clone() });
            call_index_map.push(("v2_token0", *pool_addr, PoolType::V2));
            calls.push(Call { target: *pool_addr, call_data: v2_token1_calldata.clone() });
            call_index_map.push(("v2_token1", *pool_addr, PoolType::V2));
        }
        
        let total_calls = calls.len();
        if total_calls == 0 {
            // ✅ FIX: Si TODO vino del cache, igual devolvemos estados "fresh" para el block loop.
            // Antes devolvíamos mapas vacíos y eso forzaba fallback a HotPoolManager + BatchQuote (QuoterV2) en el hot path.
            return Ok(FreshPoolStates {
                v3_states: cached_v3_states,
                v2_states: cached_v2_states,
                v2_tokens: cached_v2_tokens,
                fetch_duration: start_time.elapsed(),
                total_calls: 0,
                successful_calls: 0,
                block_number: current_block,
            });
        }
        
        info!("🔄 [JIT] Executing multicall with {} calls ({} V3 pools, {} V2 pools)",
              total_calls, total_v3, total_v2);
        
        // ✅ P0 OPTIMIZATION: Batch prioritization - touched pools get smaller batches (faster), untouched get larger batches
        // ✅ CRITICAL: Limit to max 2 multicalls per block to respect 3 RPC calls E2E limit (BlockParser: 1, JIT: 1-2)
        const MAX_MULTICALLS_PER_BLOCK: usize = 2;
        
        // ✅ P0 OPTIMIZATION: Create batches with prioritization (touched pools first)
        // Process touched pools in small batches (10-20 calls) for faster response
        // Process untouched pools in larger batches (up to 200 calls) after touched pools
        // Note: We need to rebuild touched/untouched calls from the combined calls vector
        // by tracking which pools were touched vs untouched
        let mut chunks: Vec<(usize, Vec<Call>, Vec<(&str, Address, PoolType)>, usize, bool)> = Vec::new();
        let mut chunk_idx = 0;
        
        // Split calls back into touched and untouched based on the original separation
        // We'll process them in order: first touched (small batches), then untouched (large batches)
        // Note: touched_count and untouched_count are saved before moving the vectors
        
        // Process touched pools first (small batches, high priority)
        if touched_count > 0 {
            let touched_calls_slice = &calls[0..touched_count];
            let touched_map_slice = &call_index_map[0..touched_count];
            let touched_chunks: Vec<_> = touched_calls_slice
                .chunks(touched_batch_size)
                .zip(touched_map_slice.chunks(touched_batch_size))
                .map(|(c, i)| {
                    let idx = chunk_idx;
                    chunk_idx += 1;
                    (idx, c.to_vec(), i.to_vec(), c.len(), true) // true = touched
                })
                .collect();
            chunks.extend(touched_chunks);
        }
        
        // Process untouched pools after (larger batches, lower priority)
        if untouched_count > 0 {
            let untouched_calls_slice = &calls[touched_count..touched_count + untouched_count];
            let untouched_map_slice = &call_index_map[touched_count..touched_count + untouched_count];
            let untouched_chunks: Vec<_> = untouched_calls_slice
                .chunks(untouched_batch_size)
                .zip(untouched_map_slice.chunks(untouched_batch_size))
                .map(|(c, i)| {
                    let idx = chunk_idx;
                    chunk_idx += 1;
                    (idx, c.to_vec(), i.to_vec(), c.len(), false) // false = untouched
                })
                .collect();
            chunks.extend(untouched_chunks);
        }
        
        // Limit total chunks to MAX_MULTICALLS_PER_BLOCK
        let chunks_to_process = std::cmp::min(chunks.len(), MAX_MULTICALLS_PER_BLOCK);
        if chunks.len() > MAX_MULTICALLS_PER_BLOCK {
            warn!(
                "⚠️ [JIT] Limiting multicalls to {} (from {} chunks) to respect 3 RPC calls/block limit",
                MAX_MULTICALLS_PER_BLOCK,
                chunks.len()
            );
        }
        
        // ✅ P0 OPTIMIZATION: Parallelism based on chunk count and priority
        // Process touched chunks first (they're smaller and faster)
        // Then process untouched chunks (they can wait)
        let max_parallelism = if chunks.len() == 1 && total_calls <= 50 {
            // Solo 1 chunk pequeño: ejecutar secuencialmente (overhead no vale la pena)
            1
        } else {
            // Múltiples chunks o chunk grande: usar paralelismo
            std::cmp::min(
                MAX_MULTICALLS_PER_BLOCK,
                std::cmp::min(
                    std::cmp::min(chunks.len(), 4), // Paralelismo hasta 4 chunks
                    std::cmp::min(4, self.settings.performance.max_concurrent_requests_per_host)
                )
            )
        };
        
        // Limit total chunks processed to MAX_MULTICALLS_PER_BLOCK
        let chunks_to_process = std::cmp::min(chunks.len(), MAX_MULTICALLS_PER_BLOCK);
        
        if chunks.len() > MAX_MULTICALLS_PER_BLOCK {
            warn!(
                "⚠️ [JIT] Limiting multicalls to {} (from {} chunks) to respect 3 RPC calls/block limit (BlockParser: 1, JIT: 1-2)",
                MAX_MULTICALLS_PER_BLOCK,
                chunks.len()
            );
        }
        // ✅ FIX: Timeout más conservador para evitar 84.9% failures
        // El timeout anterior (500-1000ms) era muy agresivo y causaba muchos timeouts
        // Base: 800ms, ajustado por tamaño de batch (más calls = más tiempo necesario)
        // ✅ SPRINT ESTABILIZACIÓN: Hard-Timeout de 100ms estricto para RPC
        // Si el RPC excede 100ms, abortar inmediatamente para evitar procesar datos stale
        // Esto fuerza a ver que el RPC actual no sirve y necesita nodo local
        const HARD_RPC_TIMEOUT_MS: u64 = 100;
        let hard_timeout = std::time::Duration::from_millis(HARD_RPC_TIMEOUT_MS);
        
        info!("⏱️ [JIT] Using hard timeout: {}ms (RPC must respond within this limit)", HARD_RPC_TIMEOUT_MS);
        
        let multicall_start = Instant::now();
        let mut results = Vec::new();
        
        // ✅ MEJORA: Calcular tamaño del payload del multicall
        let payload_size_bytes = calls.iter()
            .map(|call| call.call_data.len())
            .sum::<usize>();
        
        let pools_requested = total_v3 + total_v2;
        
        // ✅ SPRINT ESTABILIZACIÓN: Identificar pools solicitados para comparación
        let pools_requested_set: HashSet<Address> = touched_v3_pools.iter()
            .map(|m| m.address)
            .chain(touched_v2_pools.iter().map(|m| m.address))
            .chain(untouched_v3_pools.iter().map(|m| m.address))
            .chain(untouched_v2_pools.iter().map(|m| m.address))
            .collect();
        
        use futures::future::join_all;
        
        // ✅ FASE 1.2: Batching adaptativo simplificado
        // ✅ CRITICAL FIX: Preservar orden de chunks al procesar en paralelo
        // Procesar chunks con el optimal_batch_size calculado
        // Si falla, el multicall interno manejará el error y retornará parcial
        for chunk_group in chunks[..chunks_to_process].chunks(max_parallelism) {
            let mut tasks = Vec::new();
            // ✅ CRITICAL FIX: Guardar número de calls por chunk para placeholders
            let mut chunk_sizes = Vec::new();
            for (chunk_idx, sub_calls, _sub_index, num_calls, is_touched_chunk) in chunk_group {
                // ✅ P0 OPTIMIZATION: Use different batch size based on priority
                let batch_size = if *is_touched_chunk {
                    touched_batch_size // Small batch for touched pools (faster)
                } else {
                    untouched_batch_size // Large batch for untouched pools
                };
                
                let mc = Multicall::new(
                    Arc::clone(&provider),
                    self.multicall_address,
                    batch_size,
                );
                let sub_calls_clone = sub_calls.clone();
                let chunk_idx_clone = *chunk_idx;
                let num_calls_clone = *num_calls;
                chunk_sizes.push((chunk_idx_clone, num_calls_clone));
                // ✅ SPRINT ESTABILIZACIÓN: Capturar hard_timeout para usar en closure
                let hard_timeout_clone = hard_timeout;
                tasks.push(async move {
                    // ✅ SPRINT ESTABILIZACIÓN: Usar hard timeout de 100ms
                    let result = tokio::time::timeout(hard_timeout_clone, mc.run(sub_calls_clone, None)).await;
                    (chunk_idx_clone, result)
                });
            }
            
            let task_results = join_all(tasks).await;
            
            // ✅ CRITICAL FIX: Ordenar resultados por chunk_idx para preservar orden original
            let mut sorted_results: Vec<(usize, Result<Result<Vec<ethers::types::Bytes>, anyhow::Error>, tokio::time::error::Elapsed>)> = task_results.into_iter().collect();
            sorted_results.sort_by_key(|(chunk_idx, _)| *chunk_idx);
            
            // ✅ CRITICAL FIX: Crear mapa de tamaños por chunk_idx
            let _chunk_size_map: HashMap<usize, usize> = chunk_sizes.into_iter().collect();
            
            for (chunk_idx, task_result) in sorted_results {
                match task_result {
                    Ok(Ok(sub_results)) => {
                        if sub_results.is_empty() {
                            warn!("🚨 [JIT] Sub-batch returned ZERO results");
                            // ✅ FIX: NO agregar placeholders vacíos si el chunk devolvió cero resultados
                            // Esto evita que el parsing intente procesar datos vacíos que causan 11% pools retornados
                            // El parsing manejará la ausencia de resultados correctamente
                        } else {
                            results.extend(sub_results);
                        }
                    }
                    Ok(Err(e)) => {
                        error!("❌ [JIT] Sub-batch multicall failed: {:?}", e);
                        self.rpc_pool.report_failure(&provider);
                        // ✅ FIX: NO agregar placeholders vacíos cuando un chunk falla completamente
                        // Esto mejora el parsing correctness al no intentar parsear datos vacíos
                        warn!("⚠️ [JIT] Skipping failed chunk {} (no placeholders added)", chunk_idx);
                        warn!("⚠️ [JIT] Continuing with remaining chunks after sub-batch failure");
                    }
                    Err(_timeout) => {
                        // ✅ SPRINT ESTABILIZACIÓN: Hard timeout de 100ms excedido - abortar bloque
                        error!("⏱️ [JIT] Sub-batch timed out after {}ms (HARD TIMEOUT) - RPC too slow, aborting block processing", HARD_RPC_TIMEOUT_MS);
                        self.rpc_pool.report_failure(&provider);
                        // ✅ SPRINT ESTABILIZACIÓN: No continuar con chunks restantes si timeout
                        // Retornar error inmediatamente para abortar procesamiento del bloque
                        return Err(anyhow::anyhow!("RPC timeout: multicall exceeded {}ms hard limit. RPC is too slow for HFT. Consider using local node.", HARD_RPC_TIMEOUT_MS));
                    }
                }
            }
        }
        
        let multicall_duration = multicall_start.elapsed();
        
        if results.len() != call_index_map.len() {
            warn!("⚠️ [JIT] Multicall returned incomplete results: {} of {} calls",
                  results.len(), call_index_map.len());
            // No retornar error aquí, continuar con los resultados parciales
        }
        
        self.rpc_pool.report_success(&provider, multicall_duration);
        
        // ✅ SPRINT ESTABILIZACIÓN: Identificar pools solicitados para comparación
        let _pools_requested_set: HashSet<Address> = touched_v3_pools.iter()
            .map(|m| m.address)
            .chain(touched_v2_pools.iter().map(|m| m.address))
            .chain(untouched_v3_pools.iter().map(|m| m.address))
            .chain(untouched_v2_pools.iter().map(|m| m.address))
            .collect();
        
        // Parse results
        let (fetched_v3_states, fetched_v2_states, fetched_v2_tokens, successful_calls, failed_pools) = self.parse_multicall_results(
            &results,
            &call_index_map,
            &provider,
            &_pools_requested_set,
        ).await?;
        
        // ✅ SPRINT ESTABILIZACIÓN: Trackear pools que fallaron en corruption tracker
        for pool_addr in &failed_pools {
            // Si un pool falla 3+ veces consecutivas, marcarlo como corrupto por 24 horas
            if GLOBAL_CORRUPTION_TRACKER.try_register_multicall_failure(*pool_addr) {
                warn!("🔴 [JIT] Pool {:?} failed 3+ times in multicall, blacklisted for 24h", pool_addr);
            }
        }
        
        // ✅ MEJORA: Calcular pools devueltos vs solicitados
        let pools_returned = fetched_v3_states.len() + fetched_v2_states.len();
        
        // ✅ FIX: Criterio de éxito más flexible - ajustar según cantidad de pools solicitados
        // Para muy pocos pools (<5), ser más permisivo (evitar marcar como fallido cuando hay 1 pool y falla)
        // Para muchos pools, mantener threshold de 10%
        let success_threshold = if pools_requested <= 5 {
            // Para muy pocos pools: éxito si retornamos al menos 1 pool O si tenemos >50% de resultados presentes
            // Esto evita marcar como fallido cuando hay 1 pool solicitado y falla (retorna 0)
            std::cmp::max(1, pools_requested / 2) // Al menos la mitad, mínimo 1
        } else {
            // Para muchos pools: mantener threshold de 10%
            std::cmp::max(1, (pools_requested as f64 * 0.10) as usize)
        };
        let has_sufficient_results = results.len() >= call_index_map.len() * 9 / 10; // Al menos 90% de resultados presentes
        let has_sufficient_pools = pools_returned >= success_threshold;
        // Considerar éxito si tenemos suficientes resultados Y suficientes pools
        let success = has_sufficient_results && has_sufficient_pools;
        
        // ✅ MEJORA: Logging detallado de resultados
        if pools_returned < pools_requested {
            let return_ratio = if pools_requested > 0 {
                (pools_returned as f64 / pools_requested as f64) * 100.0
            } else {
                0.0
            };
            info!("📊 [JIT] Multicall results: {} pools returned / {} requested ({:.1}%), {} results / {} calls",
                  pools_returned, pools_requested, return_ratio, results.len(), call_index_map.len());
        }
        
        // ✅ MEJORA: Registrar RPC call con payload size y pools info
        if let Some(ref recorder) = self.flight_recorder {
            // Obtener endpoint del provider (aproximado)
            let endpoint = format!("{:?}", provider);
            
            record_rpc_call!(
                recorder,
                endpoint,
                "multicall",
                multicall_start,
                success,
                current_block,
                payload_size_bytes,
                pools_requested,
                pools_returned
            );
            
            // ✅ MEJORA: Si hay discrepancia, registrar error
            if pools_returned < pools_requested {
                record_error!(
                    recorder,
                    "jit_state_fetcher",
                    "incomplete_multicall",
                    format!("Multicall returned {} pools but {} were requested", pools_returned, pools_requested),
                    serde_json::json!({
                        "pools_requested": pools_requested,
                        "pools_returned": pools_returned,
                        "v3_states_fetched": fetched_v3_states.len(),
                        "v2_states_fetched": fetched_v2_states.len(),
                        "payload_size_bytes": payload_size_bytes,
                        "results_count": results.len(),
                        "calls_count": call_index_map.len()
                    }),
                    current_block
                );
            }
        }
        
        // 🚀 RPC OPTIMIZATION: Update cache with new states and calculate state hashes
        // ✅ SOLUCIÓN 2: Verificar si el hash de estado cambió antes de actualizar
        let cache_size_before_update = self.state_cache.len();
        let mut new_cache_entries = 0;
        let mut updated_cache_entries = 0;
        let mut hash_unchanged_count = 0; // Pools cuyo hash no cambió
        
        for (pool_addr, v3_state) in &fetched_v3_states {
            let new_merkle_root = Self::calculate_merkle_root(Some(v3_state), None, current_block);
            
            // ✅ SOLUCIÓN 2: Verificar si el hash cambió
            let hash_changed = if let Some(cached) = self.state_cache.get(pool_addr) {
                let old_hash = cached.value().merkle_root;
                old_hash != new_merkle_root
            } else {
                true // Nuevo pool, hash "cambió" (no había antes)
            };
            
            let was_present = self.state_cache.contains_key(pool_addr);
            self.state_cache.insert(*pool_addr, StateCacheCachedPoolState {
                v3_state: Some(v3_state.clone()),
                v2_state: None,
                v2_token0: None,
                v2_token1: None,
                merkle_root: new_merkle_root,
                block_number: current_block,
                last_updated: std::time::Instant::now(), // ✅ FASE 4: Timestamp para validación de cache
                touched: false,
            });
            
            if was_present {
                updated_cache_entries += 1;
                if !hash_changed {
                    hash_unchanged_count += 1;
                }
            } else {
                new_cache_entries += 1;
            }
        }
        for (pool_addr, v2_state) in &fetched_v2_states {
            let new_merkle_root = Self::calculate_merkle_root(None, Some(v2_state), current_block);
            
            // ✅ SOLUCIÓN 2: Verificar si el hash cambió
            let hash_changed = if let Some(cached) = self.state_cache.get(pool_addr) {
                let old_hash = cached.value().merkle_root;
                old_hash != new_merkle_root
            } else {
                true // Nuevo pool, hash "cambió" (no había antes)
            };
            
            let was_present = self.state_cache.contains_key(pool_addr);
            let (token0_opt, token1_opt) = fetched_v2_tokens
                .get(pool_addr)
                .map(|(a, b)| (Some(*a), Some(*b)))
                .unwrap_or((None, None));
            self.state_cache.insert(*pool_addr, StateCacheCachedPoolState {
                v3_state: None,
                v2_state: Some(*v2_state),
                v2_token0: token0_opt,
                v2_token1: token1_opt,
                merkle_root: new_merkle_root,
                block_number: current_block,
                last_updated: std::time::Instant::now(), // ✅ FASE 4: Timestamp para validación de cache
                touched: false,
            });
            
            if was_present {
                updated_cache_entries += 1;
                if !hash_changed {
                    hash_unchanged_count += 1;
                }
            } else {
                new_cache_entries += 1;
            }
        }
        
        let cache_size_after_update = self.state_cache.len();
        info!("📊 [JIT Cache] Updated cache: {} new, {} updated ({} hash_unchanged), cache_size: {} -> {} (block={})", 
               new_cache_entries, updated_cache_entries, hash_unchanged_count, 
               cache_size_before_update, cache_size_after_update, current_block);

        // ✅ Persist V2 token metadata even if we only fetched token0/token1 (token-only pools).
        for (pool_addr, (t0, t1)) in &fetched_v2_tokens {
            if let Some(mut entry) = self.state_cache.get_mut(pool_addr) {
                entry.v2_token0 = Some(*t0);
                entry.v2_token1 = Some(*t1);
            } else {
                // Rare: token metadata fetched but no cache entry existed yet.
                self.state_cache.insert(*pool_addr, StateCacheCachedPoolState {
                    v3_state: None,
                    v2_state: None,
                    v2_token0: Some(*t0),
                    v2_token1: Some(*t1),
                    merkle_root: [0u8; 32],
                    block_number: current_block,
                    last_updated: std::time::Instant::now(),
                    touched: false,
                });
                new_cache_entries += 1;
            }
        }
        
        // Store cached counts before moving
        let cached_count = cached_v3_states.len() + cached_v2_states.len();
        
        // Combine cached and fetched states
        let mut v3_states = cached_v3_states;
        let mut v2_states = cached_v2_states;
        let mut v2_tokens = cached_v2_tokens;
        v3_states.extend(fetched_v3_states);
        v2_states.extend(fetched_v2_states);
        v2_tokens.extend(fetched_v2_tokens);
        
        let _fetch_duration = start_time.elapsed();
        
        // ✅ OPTIMIZATION: Registrar métricas RPC
        let rpc_calls_used = if pools_to_fetch.is_empty() {
            0.0 // No RPC calls if all cached
        } else {
            (total_calls as f64 / self.batch_size as f64).ceil()
        };
        crate::metrics::increment_rpc_call("mvp_runner");
        crate::metrics::set_rpc_calls_per_block("mvp_runner", rpc_calls_used);
        crate::metrics::record_rpc_call_latency("mvp_runner", "jit_multicall", multicall_duration);
        
        let fetch_duration = start_time.elapsed();
        
        info!("✅ [JIT] Fetched {} states ({} V3, {} V2) in {:?} (multicall: {:?}, {} successful calls, {} RPC calls, {} cached)",
              v3_states.len() + v2_states.len(),
              v3_states.len(),
              v2_states.len(),
              fetch_duration,
              multicall_duration,
              successful_calls,
              rpc_calls_used,
              cached_count);
        
        // ✅ FLIGHT RECORDER: Registrar fin de JIT fetch con metadata detallada
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "jit_fetch_internal", start_time, serde_json::json!({
                "v3_states": v3_states.len(),
                "v2_states": v2_states.len(),
                "cached_count": cached_count,
                "cache_hits": cached_count,
                "cache_misses": pools_to_fetch.len(),
                "total_pools": pool_metadata.len(),
                "pools_to_fetch": pools_to_fetch.len(),
                "total_calls": if pools_to_fetch.is_empty() { 0 } else { total_calls },
                "successful_calls": if pools_to_fetch.is_empty() { cached_count } else { successful_calls },
                "cache_hit_rate": if pool_metadata.is_empty() { 0.0 } else {
                    (cached_count as f64 / pool_metadata.len() as f64) * 100.0
                },
                "chunks_count": chunks.len(),
                "chunks_processed": chunks_to_process,
                "max_parallelism": max_parallelism,
                "touched_batch_size": touched_batch_size,
                "untouched_batch_size": untouched_batch_size,
                "timeout_ms": HARD_RPC_TIMEOUT_MS,
                "multicall_duration_ms": multicall_duration.as_millis(),
                "payload_size_bytes": payload_size_bytes,
                "pools_requested": pools_requested,
                "pools_returned": pools_returned,
                "cache_size_before": cache_size_before_update,
                "cache_size_after": cache_size_after_update,
                // ✅ P0 OPTIMIZATION: Batch prioritization metrics
                "touched_pools_count": touched_v3_pools.len() + touched_v2_pools.len(),
                "untouched_pools_count": untouched_v3_pools.len() + untouched_v2_pools.len(),
                "touched_batch_size": touched_batch_size,
                "untouched_batch_size": untouched_batch_size,
                // ✅ P0 OPTIMIZATION: Cache validation metrics
                "merkle_cache_enabled": true,
                "fuzzy_block_matching": self.settings.performance.jit_cache_tolerance_blocks,
                "new_cache_entries": new_cache_entries,
                "updated_cache_entries": updated_cache_entries
            }), current_block);
        }
        
        Ok(FreshPoolStates {
            v3_states,
            v2_states,
            v2_tokens,
            fetch_duration,
            total_calls: if pools_to_fetch.is_empty() { 0 } else { total_calls },
            successful_calls: if pools_to_fetch.is_empty() { cached_count } else { successful_calls },
            block_number: current_block,
        })
    }
    
    /// Decode slot0 with fallback for different ABI versions
    /// Attempts full ABI (7 values) first, then minimal ABI (2 values) as fallback
    fn decode_slot0_flexible(
        &self,
        bytes: &[u8],
        slot0_fn: &ethers::abi::Function,
    ) -> Result<(U256, i32)> {
        // Intento 1: ABI completo (7 valores - Uniswap V3 actual)
        // slot0() returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, 
        //                   uint16 observationCardinality, uint16 observationCardinalityNext, 
        //                   uint8 feeProtocol, bool unlocked)
        if bytes.len() == 224 { // 32 * 7
            match slot0_fn.decode_output(bytes) {
                Ok(decoded) if decoded.len() >= 2 => {
                    if let (Some(sqrt_price), Some(tick_token)) = (
                        decoded.get(0).and_then(|t| t.clone().into_uint()),
                        decoded.get(1).and_then(|t| t.clone().into_int()),
                    ) {
                        // Convert tick from int24 (sign extend from 24 bits)
                        let tick_i32 = if tick_token.bit(23) {
                            // Negative: sign extend
                            let mask = U256::from(0xFFFFFF);
                            -((((!tick_token & mask) + 1) & mask).as_u32() as i32)
                        } else {
                            tick_token.as_u32() as i32
                        };
                        return Ok((sqrt_price, tick_i32));
                    }
                }
                Ok(_) => {
                    debug!("Full ABI decode returned insufficient values");
                }
                Err(e) => {
                    debug!("Full ABI decode failed: {}", e);
                }
            }
        }
        
        // Intento 2: ABI mínimo (2 valores - versiones antiguas o respuestas truncadas)
        if bytes.len() >= 64 { // 32 * 2
            // ✅ FLIGHT RECORDER: Registrar fallback a ABI mínimo
            if let Some(ref recorder) = self.flight_recorder {
                record_decision!(
                    recorder,
                    "jit_fetcher",
                    "decode_fallback",
                    "using_minimal_abi",
                    serde_json::json!({
                        "bytes_len": bytes.len(),
                        "expected_full_abi": 224
                    })
                );
            }
            
            // Decodificar manualmente los primeros 2 valores
            if bytes.len() >= 64 {
                let sqrt_price = U256::from_big_endian(&bytes[0..32]);
                let tick_bytes = &bytes[32..64];
                let tick_u256 = U256::from_big_endian(tick_bytes);
                
                // Convert tick from int24 (sign extend from 24 bits)
                let tick_i32 = if tick_u256.bit(23) {
                    // Negative: sign extend
                    let mask = U256::from(0xFFFFFF);
                    -((((!tick_u256 & mask) + 1) & mask).as_u32() as i32)
                } else {
                    tick_u256.as_u32() as i32
                };
                
                return Ok((sqrt_price, tick_i32));
            }
        }
        
        Err(anyhow::anyhow!("Cannot decode slot0: {} bytes (expected 224 for full ABI, 64+ for minimal)", bytes.len()))
    }

    /// Parse multicall results into V3 and V2 states
    /// Uses ABI functions for proper decoding (same logic as UnifiedStateFetcher)
    /// ✅ SPRINT ESTABILIZACIÓN: Retorna también lista de pools que fallaron
    async fn parse_multicall_results(
        &self,
        results: &[ethers::types::Bytes],
        call_index_map: &[(&str, Address, PoolType)],
        provider: &Arc<Provider<Http>>,
        pools_requested: &HashSet<Address>,
    ) -> Result<(HashMap<Address, V3PoolState>, HashMap<Address, (U256, U256)>, HashMap<Address, (Address, Address)>, usize, Vec<Address>)> {
        let mut v3_states: HashMap<Address, V3PoolState> = HashMap::new();
        let mut v2_states: HashMap<Address, (U256, U256)> = HashMap::new();
        let mut v2_tokens: HashMap<Address, (Address, Address)> = HashMap::new();
        let mut successful_calls = 0;
        // ✅ SPRINT ESTABILIZACIÓN: Trackear pools que fallaron
        let mut failed_pools: Vec<Address> = Vec::new();
        let mut pools_successfully_parsed: HashSet<Address> = HashSet::new();
        
        // Track V3 state per pool (partial states)
        #[derive(Default)]
        struct PartialV3State {
            sqrt_price_x96: Option<U256>,
            tick: Option<i64>,
            liquidity: Option<u128>,
        }
        let mut v3_partial_states: HashMap<Address, PartialV3State> = HashMap::new();
        
        // Create dummy contracts for ABI decoding
        let dummy_v3 = UniswapV3Pool::new(Address::zero(), Arc::clone(provider));
        let slot0_fn = dummy_v3.abi().function("slot0")?;
        let liquidity_fn = dummy_v3.abi().function("liquidity")?;
        
        let dummy_v2 = IUniswapV2Pair::new(Address::zero(), Arc::clone(provider));
        let reserves_fn = dummy_v2.abi().function("getReserves")?;
        let token0_fn = dummy_v2.abi().function("token0")?;
        let token1_fn = dummy_v2.abi().function("token1")?;
        
        // NOTE: unified_state_fetcher removed - use direct ERC20 contract instead
        use crate::contracts::Erc20;
        let dummy_erc20 = Erc20::new(Address::zero(), Arc::clone(provider));
        let balance_fn = dummy_erc20.abi().function("balanceOf")?;
        
        // Parse results
        #[derive(Default)]
        struct PartialV2Tokens {
            token0: Option<Address>,
            token1: Option<Address>,
        }
        let mut v2_partial_tokens: HashMap<Address, PartialV2Tokens> = HashMap::new();

        for (idx, result_data) in results.iter().enumerate() {
            if idx >= call_index_map.len() {
                break;
            }
            
            let (call_type, pool_addr, pool_type) = &call_index_map[idx];
            
            if result_data.0.is_empty() {
                warn!("⚠️ [JIT] Empty result for {} pool {:?}", call_type, pool_addr);
                // Mark V2 pools as corrupted when getReserves fails/returns empty in multicall.
                if *call_type == "v2_reserves" && !GLOBAL_CORRUPTION_TRACKER.is_corrupted(*pool_addr) {
                    GLOBAL_CORRUPTION_TRACKER.mark_corrupted(*pool_addr, "jit_v2_reserves_empty");
                }
                continue;
            }
            
            match (*call_type, *pool_type) {
                ("v3_slot0", PoolType::V3) => {
                    // Log raw bytes para debugging
                    if result_data.0.len() != 224 && result_data.0.len() != 64 {
                        warn!("⚠️ [JIT] V3 slot0 unexpected length: {} (expected 224 for 7 values or 64 for 2 values) for pool {:?}", 
                              result_data.0.len(), pool_addr);
                    }
                    
                    // Usar decode_slot0_flexible con fallback para diferentes versiones de ABI
                    match self.decode_slot0_flexible(&result_data.0, &slot0_fn) {
                        Ok((sqrt_price, tick_i32)) => {
                            let partial = v3_partial_states
                                .entry(*pool_addr)
                                .or_insert_with(PartialV3State::default);
                            partial.sqrt_price_x96 = Some(sqrt_price);
                            partial.tick = Some(tick_i32 as i64);
                            successful_calls += 1;
                        }
                        Err(e) => {
                            warn!("⚠️ [JIT] V3 slot0 decoding failed for {:?}: {:?}. Raw bytes (first 32): {:?}", 
                                  pool_addr, e, 
                                  if result_data.0.len() >= 32 { 
                                      hex::encode(&result_data.0[..32]) 
                                  } else { 
                                      "insufficient bytes".to_string() 
                                  });
                        }
                    }
                }
                ("v3_liquidity", PoolType::V3) => {
                    match liquidity_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let Some(liquidity_u256) = decoded[0].clone().into_uint() {
                                let liquidity_result: Option<u128> = liquidity_u256.try_into().ok();
                                if let Some(liquidity) = liquidity_result {
                                    if liquidity > 0 {
                                        let partial = v3_partial_states
                                            .entry(*pool_addr)
                                            .or_insert_with(PartialV3State::default);
                                        partial.liquidity = Some(liquidity);
                                        successful_calls += 1;
                                    } else {
                                        warn!("⚠️ [JIT] V3 pool {:?} has zero liquidity", pool_addr);
                                    }
                                } else {
                                    warn!("⚠️ [JIT] V3 liquidity overflow for {:?}", pool_addr);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️ [JIT] V3 liquidity decoding failed for {:?}: {:?}", pool_addr, e);
                        }
                    }
                }
                ("v3_balance0", PoolType::V3) | ("v3_balance1", PoolType::V3) => {
                    // Balances are optional, just track success
                    match balance_fn.decode_output(&result_data.0) {
                        Ok(_) => {
                            successful_calls += 1;
                        }
                        Err(_) => {
                            // Balance decoding failure is not critical
                        }
                    }
                }
                ("v2_reserves", PoolType::V2) => {
                    match reserves_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let (Some(reserve0), Some(reserve1)) = (
                                decoded[0].clone().into_uint(),
                                decoded[1].clone().into_uint(),
                            ) {
                                if !reserve0.is_zero() && !reserve1.is_zero() {
                                    v2_states.insert(*pool_addr, (reserve0, reserve1));
                                    pools_successfully_parsed.insert(*pool_addr);
                                    successful_calls += 1;
                                } else {
                                    warn!("⚠️ [JIT] V2 pool {:?} has zero reserves", pool_addr);
                                    if !GLOBAL_CORRUPTION_TRACKER.is_corrupted(*pool_addr) {
                                        GLOBAL_CORRUPTION_TRACKER.mark_corrupted(*pool_addr, "jit_v2_zero_reserves");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️ [JIT] V2 reserves decoding failed for {:?}: {:?}", pool_addr, e);
                            if !GLOBAL_CORRUPTION_TRACKER.is_corrupted(*pool_addr) {
                                GLOBAL_CORRUPTION_TRACKER.mark_corrupted(*pool_addr, "jit_v2_reserves_decode_failed");
                            }
                        }
                    }
                }
                ("v2_token0", PoolType::V2) => {
                    match token0_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let Some(t0) = decoded.get(0).and_then(|t| t.clone().into_address()) {
                                let partial = v2_partial_tokens.entry(*pool_addr).or_insert_with(PartialV2Tokens::default);
                                partial.token0 = Some(t0);
                                successful_calls += 1;
                            }
                        }
                        Err(e) => warn!("⚠️ [JIT] V2 token0 decoding failed for {:?}: {:?}", pool_addr, e),
                    }
                }
                ("v2_token1", PoolType::V2) => {
                    match token1_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let Some(t1) = decoded.get(0).and_then(|t| t.clone().into_address()) {
                                let partial = v2_partial_tokens.entry(*pool_addr).or_insert_with(PartialV2Tokens::default);
                                partial.token1 = Some(t1);
                                successful_calls += 1;
                            }
                        }
                        Err(e) => warn!("⚠️ [JIT] V2 token1 decoding failed for {:?}: {:?}", pool_addr, e),
                    }
                }
                _ => {
                    warn!("⚠️ [JIT] Unknown call type: {} for pool {:?}", call_type, pool_addr);
                }
            }
        }
        
        // ✅ FIX: Mejorar logging de resultados vacíos vs válidos
        let mut empty_results_count = 0;
        let mut valid_results_count = 0;
        let mut total_result_bytes = 0;
        
        for result_data in results.iter() {
            if result_data.0.is_empty() {
                empty_results_count += 1;
            } else {
                valid_results_count += 1;
                total_result_bytes += result_data.0.len();
            }
        }
        
        // ✅ MEJORA: Logging detallado de resultados
        if empty_results_count > 0 || valid_results_count < results.len() {
            let empty_ratio = if results.len() > 0 {
                (empty_results_count as f64 / results.len() as f64) * 100.0
            } else {
                0.0
            };
            let avg_bytes_per_result = if valid_results_count > 0 {
                total_result_bytes / valid_results_count
            } else {
                0
            };
            info!("📊 [JIT] Parsing results: {} empty / {} total ({:.1}% empty), {} valid, avg {} bytes/result",
                  empty_results_count, results.len(), empty_ratio, valid_results_count, avg_bytes_per_result);
        }
        
        // ✅ FIX: Optimizar parsing - intentar usar estados parciales cuando sea posible
        // Build complete V3 states from partial data
        let mut incomplete_v3_count = 0;
        let mut v3_missing_slot0 = 0;
        let mut v3_missing_liquidity = 0;
        let mut v3_partial_used = 0; // Pools que usamos con datos parciales
        let v3_partial_states_count = v3_partial_states.len();
        
        for (pool_addr, partial) in v3_partial_states {
            // ✅ FIX: Intentar construir estado completo primero
            if let (Some(sqrt_price_x96), Some(tick), Some(liquidity)) = 
                (partial.sqrt_price_x96, partial.tick, partial.liquidity) {
                v3_states.insert(pool_addr, V3PoolState {
                    sqrt_price_x96,
                    tick,
                    liquidity,
                });
                pools_successfully_parsed.insert(pool_addr);
            } else {
                // ✅ FIX: Si tenemos slot0 pero falta liquidity, intentar usar con liquidity=0 como fallback
                // Esto permite que el simulador al menos intente simular (aunque puede fallar por falta de liquidity)
                if let (Some(sqrt_price_x96), Some(tick)) = (partial.sqrt_price_x96, partial.tick) {
                    // Usar liquidity mínima (1) en vez de 0 para evitar divisiones por cero
                    let fallback_liquidity = partial.liquidity.unwrap_or(1);
                    if fallback_liquidity > 0 {
                        warn!("⚠️ [JIT] V3 pool {:?} using partial state (missing liquidity, using fallback={})", pool_addr, fallback_liquidity);
                        v3_states.insert(pool_addr, V3PoolState {
                            sqrt_price_x96,
                            tick,
                            liquidity: fallback_liquidity,
                        });
                        pools_successfully_parsed.insert(pool_addr);
                        v3_partial_used += 1;
                        continue;
                    }
                }
                
                // Si no podemos construir estado, marcar como incompleto
                incomplete_v3_count += 1;
                if partial.sqrt_price_x96.is_none() || partial.tick.is_none() {
                    v3_missing_slot0 += 1;
                }
                if partial.liquidity.is_none() {
                    v3_missing_liquidity += 1;
                }
                warn!("⚠️ [JIT] V3 pool {:?} missing required state (sqrt_price={:?}, tick={:?}, liquidity={:?})",
                      pool_addr, partial.sqrt_price_x96.is_some(), partial.tick.is_some(), partial.liquidity.is_some());
            }
        }
        
        if v3_partial_used > 0 {
            info!("📊 [JIT] Used {} partial V3 states with fallback liquidity", v3_partial_used);
        }
        
        // ✅ FLIGHT RECORDER: Registrar estadísticas de parsing
        if let Some(ref recorder) = self.flight_recorder {
            record_decision!(
                recorder,
                "jit_state_fetcher",
                "parse_multicall_results",
                "parsing_stats",
                serde_json::json!({
                    "total_results": results.len(),
                    "empty_results": empty_results_count,
                    "v3_partial_states": v3_partial_states_count,
                    "v3_complete_states": v3_states.len(),
                    "v3_incomplete_states": incomplete_v3_count,
                    "v3_missing_slot0": v3_missing_slot0,
                    "v3_missing_liquidity": v3_missing_liquidity,
                    "v3_partial_used": v3_partial_used,
                    "empty_results": empty_results_count,
                    "valid_results": valid_results_count,
                    "v2_states": v2_states.len(),
                    "successful_calls": successful_calls,
                    "calls_count": call_index_map.len()
                })
            );
        }

        // Build complete V2 tokens map from partial data
        for (pool_addr, partial) in v2_partial_tokens {
            if let (Some(t0), Some(t1)) = (partial.token0, partial.token1) {
                v2_tokens.insert(pool_addr, (t0, t1));
            }
        }
        
        // ✅ SPRINT ESTABILIZACIÓN: Identificar pools que fallaron
        // Un pool falla si fue solicitado pero no está en los estados parseados exitosamente
        for pool_addr in pools_requested {
            if !pools_successfully_parsed.contains(pool_addr) {
                failed_pools.push(*pool_addr);
                warn!("🔴 [JIT] Pool {:?} failed in multicall (requested but not returned)", pool_addr);
            }
        }
        
        if !failed_pools.is_empty() {
            info!("📊 [JIT] {} pools failed in multicall: {:?}", failed_pools.len(), 
                  failed_pools.iter().take(5).collect::<Vec<_>>());
        }
        
        Ok((v3_states, v2_states, v2_tokens, successful_calls, failed_pools))
    }
    
    /// ✅ CONSULTOR FIX: Fast path para fetch directo de un solo pool sin multicall
    /// Usar cuando solo hay 1 pool para evitar overhead de multicall y el problema
    /// de que multicall falla 100% cuando solo hay 1 pool.
    async fn fetch_single_pool_direct(
        &self,
        pool_meta: &PoolMetadata,
        provider: Arc<Provider<Http>>,
        block: Option<u64>,
    ) -> Result<Option<FreshPoolStates>> {
        let pool_addr = pool_meta.address;
        
        // ✅ P1 FIX: Verificar pool corrupto ANTES de RPC call
        use crate::pool_blacklist::GLOBAL_CORRUPTION_TRACKER; // NOTE: Renamed from pool_corruption_tracker
        if GLOBAL_CORRUPTION_TRACKER.is_corrupted(pool_addr) {
            debug!("⚠️ [JIT Fast Path] Pool {:?} is corrupted, skipping RPC call", pool_addr);
            return Ok(None);
        }
        
        match pool_meta.pool_type {
            PoolType::V3 => {
                // Construir contrato V3 - clonar provider para cada llamada
                let provider_clone1 = Arc::clone(&provider);
                let provider_clone2 = Arc::clone(&provider);
                
                // Ejecutar slot0() y liquidity() en paralelo con timeout
                // ✅ FIX: Crear bindings intermedios para evitar temporary value dropped
                let call_result = tokio::time::timeout(
                    std::time::Duration::from_millis(200), // Timeout corto para 1 pool
                    async move {
                        let pool1 = UniswapV3Pool::new(pool_addr, provider_clone1);
                        let pool2 = UniswapV3Pool::new(pool_addr, provider_clone2);
                        let slot0_binding = pool1.slot_0();
                        let liquidity_binding = pool2.liquidity();
                        let slot0_future = slot0_binding.call();
                        let liquidity_future = liquidity_binding.call();
                        tokio::try_join!(slot0_future, liquidity_future)
                    }
                ).await;
                
                match call_result {
                    Ok(Ok((slot0_result, liquidity_result))) => {
                        // ✅ CORRECCIÓN: Los resultados de .call() vienen como tuplas
                        // slot0_result es (U256, i32, u16, u16, u16, u8, bool) -> (sqrt_price_x96, tick, ...)
                        // liquidity_result es directamente u128
                        let sqrt_price_x96 = slot0_result.0;
                        let tick = slot0_result.1 as i64; // tick viene como i32, convertir a i64
                        let liquidity = liquidity_result;
                        
                        // Validar estado
                        if sqrt_price_x96.is_zero() || liquidity == 0 {
                            debug!("V3 pool {:?} has invalid state (zero price or liquidity)", pool_addr);
                            return Ok(None);
                        }
                        
                        // Retornar estado válido
                        let mut v3_states = HashMap::new();
                        v3_states.insert(pool_addr, V3PoolState {
                            sqrt_price_x96,
                            tick,
                            liquidity,
                        });
                        
                        Ok(Some(FreshPoolStates {
                            v3_states,
                            v2_states: HashMap::new(),
                            v2_tokens: HashMap::new(),
                            fetch_duration: std::time::Duration::ZERO, // Se calcula en el caller
                            total_calls: 2,
                            successful_calls: 2,
                            block_number: block.unwrap_or(0),
                        }))
                    }
                    Ok(Err(e)) => {
                        debug!("RPC call failed for V3 pool {:?}: {}. Pool likely doesn't exist.", pool_addr, e);
                        Ok(None)
                    }
                    Err(_) => {
                        debug!("Timeout fetching V3 pool {:?} (>200ms). Pool likely slow or inactive.", pool_addr);
                        Ok(None)
                    }
                }
            }
            
            PoolType::V2 => {
                // ✅ FIX: Clonar provider para cada llamada y crear contratos dentro del async block
                let provider1 = Arc::clone(&provider);
                let provider2 = Arc::clone(&provider);
                let provider3 = Arc::clone(&provider);
                
                // Ejecutar getReserves(), token0() y token1() en paralelo con timeout
                // ✅ FIX: Crear bindings intermedios para evitar temporary value dropped
                let call_result = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    async move {
                        let pair1 = IUniswapV2Pair::new(pool_addr, provider1);
                        let pair2 = IUniswapV2Pair::new(pool_addr, provider2);
                        let pair3 = IUniswapV2Pair::new(pool_addr, provider3);
                        let reserves_binding = pair1.get_reserves();
                        let token0_binding = pair2.token_0();
                        let token1_binding = pair3.token_1();
                        let reserves_future = reserves_binding.call();
                        let token0_future = token0_binding.call();
                        let token1_future = token1_binding.call();
                        tokio::try_join!(reserves_future, token0_future, token1_future)
                    }
                ).await;
                
                match call_result {
                    Ok(Ok((reserves_result, token0_result, token1_result))) => {
                        // ✅ CORRECCIÓN: Los resultados de .call() vienen como tuplas
                        // reserves_result es (u128, u128, u32) -> (reserve0, reserve1, blockTimestampLast)
                        // token0_result y token1_result son directamente Address
                        let reserve0 = U256::from(reserves_result.0);
                        let reserve1 = U256::from(reserves_result.1);
                        let token0 = token0_result;
                        let token1 = token1_result;
                        
                        // Validar reserves
                        if reserve0.is_zero() || reserve1.is_zero() {
                            debug!("V2 pool {:?} has zero reserves", pool_addr);
                            return Ok(None);
                        }
                        
                        // Retornar estado válido
                        let mut v2_states = HashMap::new();
                        v2_states.insert(pool_addr, (reserve0, reserve1));
                        
                        let mut v2_tokens = HashMap::new();
                        v2_tokens.insert(pool_addr, (token0, token1));
                        
                        Ok(Some(FreshPoolStates {
                            v3_states: HashMap::new(),
                            v2_states,
                            v2_tokens,
                            fetch_duration: std::time::Duration::ZERO, // Se calcula en el caller
                            total_calls: 3, // getReserves + token0 + token1
                            successful_calls: 3,
                            block_number: block.unwrap_or(0),
                        }))
                    }
                    Ok(Err(e)) => {
                        debug!("RPC call failed for V2 pool {:?}: {}. Pool likely doesn't exist.", pool_addr, e);
                        Ok(None)
                    }
                    Err(_) => {
                        debug!("Timeout fetching V2 pool {:?} (>200ms). Pool likely slow or inactive.", pool_addr);
                        Ok(None)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc_pool::RpcPool;
    use crate::v3_math::V3PoolState;
    use std::sync::Arc as StdArc;

    #[tokio::test]
    async fn jit_cache_only_returns_cached_states_when_total_calls_zero() {
        // No tocamos env vars globales (tests corren en paralelo). En su lugar, forzamos un HTTP URL válido
        // en una instancia local de Settings antes de construir el RpcPool.
        let mut settings_val = Settings::new().expect("Settings::new() must succeed for tests");
        settings_val.rpc.http_urls = vec!["http://localhost:8545".to_string()];
        let settings = StdArc::new(settings_val);
        let rpc_pool = StdArc::new(RpcPool::new(StdArc::clone(&settings)).expect("RpcPool::new must succeed"));

        let fetcher = JitStateFetcher::new(
            rpc_pool,
            "0xcA11bde05977b3631167028862bE2a173976CA11".parse().unwrap(),
            200,
            StdArc::clone(&settings),
        );

        let current_block = 123u64;
        let v3_pool: Address = "0x00000000000000000000000000000000000000a1".parse().unwrap();
        let v2_pool: Address = "0x00000000000000000000000000000000000000b2".parse().unwrap();

        // Seed cache with both V3 and V2 states so pools_to_fetch becomes empty.
        let v3_state = V3PoolState {
            sqrt_price_x96: U256::from(1u64) << 96,
            tick: 0,
            liquidity: 1_000_000u128,
        };
        let v2_state = (U256::from(1_000_000u64), U256::from(2_000_000u64));

        fetcher.state_cache.insert(
            v3_pool,
            StateCacheCachedPoolState {
                v3_state: Some(v3_state.clone()),
                v2_state: None,
                v2_token0: None,
                v2_token1: None,
                merkle_root: JitStateFetcher::calculate_merkle_root(Some(&v3_state), None, current_block),
                block_number: current_block,
                last_updated: std::time::Instant::now(),
                touched: false,
            },
        );
        fetcher.state_cache.insert(
            v2_pool,
            StateCacheCachedPoolState {
                v3_state: None,
                v2_state: Some(v2_state),
                v2_token0: Some("0x0000000000000000000000000000000000000002".parse().unwrap()),
                v2_token1: Some("0x0000000000000000000000000000000000000003".parse().unwrap()),
                merkle_root: JitStateFetcher::calculate_merkle_root(None, Some(&v2_state), current_block),
                block_number: current_block,
                last_updated: std::time::Instant::now(),
                touched: false,
            },
        );

        let metas = vec![
            PoolMetadata {
                address: v3_pool,
                pool_type: PoolType::V3,
                token0: Address::zero(),
                token1: Address::zero(),
                fee: Some(500),
            },
            PoolMetadata {
                address: v2_pool,
                pool_type: PoolType::V2,
                token0: Address::zero(),
                token1: Address::zero(),
                fee: None,
            },
        ];

        let states = fetcher.fetch_current_states(&metas, current_block).await.expect("fetch_current_states");
        assert_eq!(states.total_calls, 0);
        assert!(states.v3_states.contains_key(&v3_pool));
        assert!(states.v2_states.contains_key(&v2_pool));
        assert!(states.v2_tokens.contains_key(&v2_pool));
    }
}

