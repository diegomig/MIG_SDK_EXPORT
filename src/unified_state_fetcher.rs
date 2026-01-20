//! # Unified State Fetcher
//!
//! Fetches pool states for V2, V3, Curve, and Balancer pools using unified multicall.
//! Implements delta refresh (only touched pools) with periodic full refresh.
//! Includes fallback direct fetching for small pool sets and BlockNumberCache integration.

use anyhow::{anyhow, Result};
use ethers::prelude::{Http, Middleware, Provider};
use ethers::types::{Address, U256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use crate::contracts::{
    erc20::Erc20,
    i_uniswap_v2_pair::IUniswapV2Pair,
    uniswap_v3::UniswapV3Pool,
    i_balancer_v2_vault::IBalancerV2Vault,
    i_weighted_pool::IWeightedPool,
    i_curve_pool::ICurvePool,
};
use crate::data_pipeline::{DataPipeline, DataSource, NormalizedV2Reserves};
use crate::data_validator::StateQuality;
use crate::hot_pool_manager::{HotPoolManager, V3PoolSnapshot};
use crate::multicall::{Call, Multicall};
use crate::rpc_pool::{RpcPool, RpcRole};
use crate::settings::Settings;
use crate::v3_math::V3PoolState;
use crate::block_number_cache::BlockNumberCache;
use std::sync::Arc as StdArc;

/// Unified state update containing all pool state changes
#[derive(Debug, Clone)]
pub struct UnifiedStateUpdate {
    pub v3_updates: HashMap<Address, V3PoolState>,
    pub v3_balances: HashMap<Address, (U256, U256)>,
    pub v2_updates: HashMap<Address, (U256, U256)>, // (reserve0, reserve1)
    pub curve_updates: HashMap<Address, CurveState>,
    pub balancer_updates: HashMap<Address, BalancerState>,
    pub fetch_duration: std::time::Duration,
    pub total_calls: usize,
    pub successful_calls: usize,
    pub block_number: u64,
    pub v3_requested: Vec<Address>,
    pub v2_requested: Vec<Address>,
    pub curve_requested: Vec<Address>,
    pub balancer_requested: Vec<Address>,
    pub v3_partial_fail_counts: HashMap<Address, u8>,
    pub v2_partial_fail_counts: HashMap<Address, u8>,
}

/// State for Curve StableSwap pool
#[derive(Debug, Clone)]
pub struct CurveState {
    pub balances: Vec<U256>,
    pub a: U256,
    pub fee: U256,
}

/// State for Balancer Weighted pool
#[derive(Debug, Clone)]
pub struct BalancerState {
    pub tokens: Vec<Address>,
    pub balances: Vec<U256>,
    pub weights: Vec<U256>,
    pub swap_fee: U256,
}

/// Unified state fetcher with delta refresh and fallback support
pub struct UnifiedStateFetcher {
    rpc_pool: Arc<RpcPool>,
    multicall_address: Address,
    batch_size: usize,
    pipeline: DataPipeline,
    audit_mode: bool,
    settings: StdArc<Settings>,
    block_number_cache: Option<Arc<BlockNumberCache>>,
}

const DIRECT_FALLBACK_V3_LIMIT: usize = 24;
const DIRECT_FALLBACK_V2_LIMIT: usize = 24;
const DIRECT_FALLBACK_CURVE_LIMIT: usize = 12;
const DIRECT_FALLBACK_BALANCER_LIMIT: usize = 12;
const V3_CALLS_PER_POOL: usize = 4; // slot0 + liquidity + balance0 + balance1

impl UnifiedStateFetcher {
    pub fn new(
        rpc_pool: Arc<RpcPool>,
        multicall_address: Address,
        batch_size: usize,
        audit_mode: bool,
        settings: StdArc<Settings>,
    ) -> Self {
        Self {
            rpc_pool,
            multicall_address,
            batch_size,
            pipeline: DataPipeline::new(audit_mode),
            audit_mode,
            settings,
            block_number_cache: None,
        }
    }

    /// Set block number cache to avoid individual get_block_number() calls
    pub fn with_block_number_cache(mut self, block_number_cache: Arc<BlockNumberCache>) -> Self {
        self.block_number_cache = Some(block_number_cache);
        self
    }

    /// DELTA REFRESH: Fetch only touched pools + periodic full refresh
    /// - cycle_number: used to determine if full refresh is needed (every 10 cycles)
    /// - Returns: UnifiedStateUpdate with fetched states
    pub async fn fetch_hot_pool_states_delta(
        &mut self,
        hot_manager: &HotPoolManager,
        cycle_number: u64,
    ) -> Result<UnifiedStateUpdate> {
        let start_time = Instant::now();

        // Determine fetch strategy: delta (touched only) or full (every 10 cycles)
        let is_full_refresh = cycle_number % 10 == 0;

        let (
            v3_pools_to_refresh,
            v2_pools_to_refresh,
            curve_pools_to_refresh,
            balancer_pools_to_refresh,
        ) = if is_full_refresh {
            // FULL REFRESH: Use time-based logic
            info!(
                "🔄 FULL REFRESH (cycle {}): Using time-based refresh for all hot pools",
                cycle_number
            );
            hot_manager.get_pools_needing_refresh().await
        } else {
            // DELTA REFRESH: Only fetch touched pools
            let (v3_touched, v2_touched, curve_touched, balancer_touched) =
                hot_manager.get_touched_pools();
            info!("⚡ DELTA REFRESH (cycle {}): Fetching {} V3, {} V2, {} Curve, {} Balancer touched pools", 
                  cycle_number, v3_touched.len(), v2_touched.len(), curve_touched.len(), balancer_touched.len());
            (v3_touched, v2_touched, curve_touched, balancer_touched)
        };

        // If no pools to refresh in delta mode, skip fetch entirely
        if !is_full_refresh
            && v3_pools_to_refresh.is_empty()
            && v2_pools_to_refresh.is_empty()
            && curve_pools_to_refresh.is_empty()
            && balancer_pools_to_refresh.is_empty()
        {
            info!("⏭️  DELTA REFRESH: No touched pools, skipping fetch");
            return Ok(UnifiedStateUpdate {
                v3_updates: HashMap::new(),
                v3_balances: HashMap::new(),
                v2_updates: HashMap::new(),
                curve_updates: HashMap::new(),
                balancer_updates: HashMap::new(),
                fetch_duration: start_time.elapsed(),
                total_calls: 0,
                successful_calls: 0,
                block_number: 0,
                v3_requested: Vec::new(),
                v2_requested: Vec::new(),
                curve_requested: Vec::new(),
                balancer_requested: Vec::new(),
                v3_partial_fail_counts: HashMap::new(),
                v2_partial_fail_counts: HashMap::new(),
            });
        }

        // Continue with standard fetch logic using the selected pools
        self.fetch_pool_states_internal(
            hot_manager,
            v3_pools_to_refresh,
            v2_pools_to_refresh,
            curve_pools_to_refresh,
            balancer_pools_to_refresh,
            start_time,
        )
        .await
    }

    /// Fetch all hot pool states in a single unified multicall (LEGACY - use fetch_hot_pool_states_delta)
    pub async fn fetch_hot_pool_states(
        &mut self,
        hot_manager: &HotPoolManager,
    ) -> Result<UnifiedStateUpdate> {
        // Call delta version with cycle_number = 0 (always full refresh)
        self.fetch_hot_pool_states_delta(hot_manager, 0).await
    }

    /// Fetch states for selected pools only (optimized for BlockParser)
    pub async fn fetch_selected_pools(
        &mut self,
        hot_manager: &HotPoolManager,
        selected_pools: Option<&std::collections::HashSet<Address>>,
        cycle_number: u64,
    ) -> Result<UnifiedStateUpdate> {
        let start_time = Instant::now();

        let (v3_pools, v2_pools, curve_pools, balancer_pools) = if let Some(selected) = selected_pools {
            // Filter pools from HotPoolManager that are in selected_pools
            let (v3_all, v2_all, curve_all, balancer_all) = hot_manager.get_current_hot_addresses();
            
            let v3_filtered: Vec<Address> = v3_all.into_iter()
                .filter(|addr| selected.contains(addr))
                .collect();
            let v2_filtered: Vec<Address> = v2_all.into_iter()
                .filter(|addr| selected.contains(addr))
                .collect();
            let curve_filtered: Vec<Address> = curve_all.into_iter()
                .filter(|addr| selected.contains(addr))
                .collect();
            let balancer_filtered: Vec<Address> = balancer_all.into_iter()
                .filter(|addr| selected.contains(addr))
                .collect();
            
            info!("📊 Fetching selected pools: {} V3, {} V2, {} Curve, {} Balancer",
                  v3_filtered.len(), v2_filtered.len(), curve_filtered.len(), balancer_filtered.len());
            
            (v3_filtered, v2_filtered, curve_filtered, balancer_filtered)
        } else {
            // Normal behavior: use delta refresh
            return self.fetch_hot_pool_states_delta(hot_manager, cycle_number).await;
        };

        // If no selected pools, return empty update
        if v3_pools.is_empty() && v2_pools.is_empty() && curve_pools.is_empty() && balancer_pools.is_empty() {
            info!("⏭️ No selected pools found in HotPoolManager, skipping fetch");
            return Ok(UnifiedStateUpdate {
                v3_updates: HashMap::new(),
                v3_balances: HashMap::new(),
                v2_updates: HashMap::new(),
                curve_updates: HashMap::new(),
                balancer_updates: HashMap::new(),
                fetch_duration: start_time.elapsed(),
                total_calls: 0,
                successful_calls: 0,
                block_number: 0,
                v3_requested: Vec::new(),
                v2_requested: Vec::new(),
                curve_requested: Vec::new(),
                balancer_requested: Vec::new(),
                v3_partial_fail_counts: HashMap::new(),
                v2_partial_fail_counts: HashMap::new(),
            });
        }

        // Use internal method with filtered pools
        self.fetch_pool_states_internal(
            hot_manager,
            v3_pools,
            v2_pools,
            curve_pools,
            balancer_pools,
            start_time,
        )
        .await
    }

    /// Internal implementation of pool state fetching
    async fn fetch_pool_states_internal(
        &mut self,
        hot_manager: &HotPoolManager,
        mut v3_pools_to_refresh: Vec<Address>,
        mut v2_pools_to_refresh: Vec<Address>,
        curve_pools_to_refresh: Vec<Address>,
        balancer_pools_to_refresh: Vec<Address>,
        start_time: Instant,
    ) -> Result<UnifiedStateUpdate> {
        info!(
            "UnifiedStateFetcher: Got {} V3, {} V2, {} Curve, {} Balancer pools to refresh",
            v3_pools_to_refresh.len(),
            v2_pools_to_refresh.len(),
            curve_pools_to_refresh.len(),
            balancer_pools_to_refresh.len()
        );

        // Ensure we always refresh at least some V2 to keep reserves fresh (top 200)
        if v2_pools_to_refresh.len() < 50 {
            let (_v3_all2, v2_all2, _c2, _b2) = hot_manager.get_current_hot_addresses();
            const MIN_V2_CALLS: usize = 200;
            let need = MIN_V2_CALLS.saturating_sub(v2_pools_to_refresh.len());
            for addr in v2_all2.into_iter().take(need) {
                if !v2_pools_to_refresh.contains(&addr) {
                    v2_pools_to_refresh.push(addr);
                }
            }
            // Enforce batch budget by shrinking V3 first to preserve MIN_V2_CALLS
            let total_calls = v3_pools_to_refresh.len() * V3_CALLS_PER_POOL + v2_pools_to_refresh.len();
            if total_calls > self.batch_size {
                let v2_keep = MIN_V2_CALLS.min(v2_pools_to_refresh.len()).min(self.batch_size);
                let v3_allowed = self
                    .batch_size
                    .saturating_sub(v2_keep)
                    / V3_CALLS_PER_POOL;
                if v3_pools_to_refresh.len() > v3_allowed {
                    v3_pools_to_refresh.truncate(v3_allowed);
                }
                let remaining_for_v2 = self
                    .batch_size
                    .saturating_sub(v3_pools_to_refresh.len() * V3_CALLS_PER_POOL);
                if v2_pools_to_refresh.len() > remaining_for_v2 {
                    v2_pools_to_refresh.truncate(remaining_for_v2);
                }
            }
        }

        let max_attempts = 2usize;
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 1..=max_attempts {
            // Build unified multicall with current provider
            let (_multicall, (provider, _permit)) = self
                .rpc_pool
                .acquire_multicall(RpcRole::State, self.multicall_address, self.batch_size)
                .await?;

            let mut calls = Vec::new();
            let mut call_index_map = Vec::new();

            // V3: slot0 + liquidity + balances
            for &pool_addr in &v3_pools_to_refresh {
                let snapshot = hot_manager.v3_hot_pools.get(&pool_addr);
                let (token0, token1) = if let Some(entry) = snapshot {
                    (entry.token0, entry.token1)
                } else {
                    (Address::zero(), Address::zero())
                };

                let pool_contract = UniswapV3Pool::new(pool_addr, Arc::clone(&provider));
                calls.push(Call {
                    target: pool_addr,
                    call_data: pool_contract.slot_0().calldata().unwrap(),
                });
                call_index_map.push(("v3_slot0", pool_addr));

                calls.push(Call {
                    target: pool_addr,
                    call_data: pool_contract.liquidity().calldata().unwrap(),
                });
                call_index_map.push(("v3_liquidity", pool_addr));

                if token0 != Address::zero() {
                    let token0_contract = Erc20::new(token0, Arc::clone(&provider));
                    calls.push(Call {
                        target: token0,
                        call_data: token0_contract.balance_of(pool_addr).calldata().unwrap(),
                    });
                    call_index_map.push(("v3_balance0", pool_addr));
                }

                if token1 != Address::zero() {
                    let token1_contract = Erc20::new(token1, Arc::clone(&provider));
                    calls.push(Call {
                        target: token1,
                        call_data: token1_contract.balance_of(pool_addr).calldata().unwrap(),
                    });
                    call_index_map.push(("v3_balance1", pool_addr));
                }
            }

            // V2: reserves
            for &pool_addr in &v2_pools_to_refresh {
                let pair_contract = IUniswapV2Pair::new(pool_addr, Arc::clone(&provider));
                calls.push(Call {
                    target: pool_addr,
                    call_data: pair_contract.get_reserves().calldata().unwrap(),
                });
                call_index_map.push(("v2_reserves", pool_addr));
            }

            // Curve: balances, A, fee (support 2 tokens)
            for &pool_addr in &curve_pools_to_refresh {
                let curve_contract = ICurvePool::new(pool_addr, Arc::clone(&provider));
                let num_tokens = hot_manager
                    .curve_hot_pools
                    .get(&pool_addr)
                    .map(|entry| entry.tokens.len())
                    .unwrap_or(2);

                for i in 0..num_tokens.min(2) {
                    calls.push(Call {
                        target: pool_addr,
                        call_data: curve_contract.balances(i.into()).calldata().unwrap(),
                    });
                    call_index_map.push(("curve_balance", pool_addr));
                }

                calls.push(Call {
                    target: pool_addr,
                    call_data: curve_contract.a().calldata().unwrap(),
                });
                call_index_map.push(("curve_a", pool_addr));

                calls.push(Call {
                    target: pool_addr,
                    call_data: curve_contract.fee().calldata().unwrap(),
                });
                call_index_map.push(("curve_fee", pool_addr));
            }

            // Balancer: balances + fee
            for &pool_addr in &balancer_pools_to_refresh {
                let pool_id = hot_manager
                    .balancer_hot_pools
                    .get(&pool_addr)
                    .map(|entry| entry.pool_id)
                    .unwrap_or([0u8; 32]);

                let vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
                    .parse()
                    .unwrap();
                let vault = IBalancerV2Vault::new(vault_addr, Arc::clone(&provider));
                let pool = IWeightedPool::new(pool_addr, Arc::clone(&provider));

                calls.push(Call {
                    target: vault_addr,
                    call_data: vault.get_pool_tokens(pool_id).calldata().unwrap(),
                });
                call_index_map.push(("balancer_tokens", pool_addr));

                calls.push(Call {
                    target: pool_addr,
                    call_data: pool.get_swap_fee_percentage().calldata().unwrap(),
                });
                call_index_map.push(("balancer_fee", pool_addr));
            }

            let total_calls = calls.len();
            info!(
                "Unified state fetch attempt {}/{}: {} calls (V3={}, V2={}, Curve={}, Balancer={})",
                attempt,
                max_attempts,
                total_calls,
                v3_pools_to_refresh.len(),
                v2_pools_to_refresh.len(),
                curve_pools_to_refresh.len(),
                balancer_pools_to_refresh.len()
            );

            // Use configured batch size (max 500, as per original)
            let configured_batch = std::cmp::min(
                self.batch_size,
                std::cmp::max(500, self.settings.performance.multicall_batch_size),
            );
            let sub_batch_size = std::cmp::max(500, configured_batch);
            let max_parallelism = std::cmp::max(
                16, // Maximum 16 parallel batches (increased from 8)
                self.settings.performance.max_concurrent_requests_per_host,
            );
            let base_timeout_ms = std::cmp::max(
                800u64,
                (self.settings.performance.multicall_timeout_seconds as u64) * 1000,
            );
            let sub_batch_timeout = std::time::Duration::from_millis(base_timeout_ms);

            // Split calls into batches with call_index_map alignment
            let chunks: Vec<(Vec<Call>, Vec<(&str, Address)>)> = calls
                .chunks(sub_batch_size)
                .zip(call_index_map.chunks(sub_batch_size))
                .map(|(c, i)| (c.to_vec(), i.to_vec()))
                .collect();

            info!(
                "Split into {} sub-batches of ~{} calls (attempt {}/{})",
                chunks.len(),
                sub_batch_size,
                attempt,
                max_attempts
            );

            // Process batches in parallel (up to max_parallelism at a time) with timeouts
            use futures::future::join_all;
            let mut results = Vec::new();
            let mut partial_failure = false;

            // Process all chunks in parallel (up to max_parallelism at a time)
            for chunk_group in chunks.chunks(max_parallelism) {
                let mut tasks = Vec::new();
                for (sub_calls, _sub_index) in chunk_group {
                    let mc = Multicall::new(
                        Arc::clone(&provider),
                        self.multicall_address,
                        sub_batch_size,
                    );
                    let sub_calls_clone = sub_calls.clone();
                    tasks.push(async move {
                        tokio::time::timeout(sub_batch_timeout, mc.run(sub_calls_clone, None)).await
                    });
                }

                // Use join_all to process all tasks in parallel
                let task_results = join_all(tasks).await;
                
                for (task_idx, task_result) in task_results.into_iter().enumerate() {
                    match task_result {
                        Ok(Ok(sub_results)) => {
                            if sub_results.is_empty() {
                                tracing::error!(
                                    "🚨 Sub-batch {} returned ZERO results (attempt {}/{})",
                                    task_idx,
                                    attempt,
                                    max_attempts
                                );
                                partial_failure = true;
                            }
                            results.extend(sub_results);
                        }
                        Ok(Err(e)) => {
                            tracing::error!("❌ Sub-batch {} multicall failed: {:?}", task_idx, e);
                            partial_failure = true;
                        }
                        Err(_timeout) => {
                            tracing::error!(
                                "⏱️  Sub-batch {} timed out after {:?}",
                                task_idx,
                                sub_batch_timeout
                            );
                            partial_failure = true;
                        }
                    }
                }
            }

            if results.is_empty() || results.len() != call_index_map.len() || partial_failure {
                warn!(
                    attempt,
                    max_attempts,
                    total_calls,
                    results_len = results.len(),
                    partial_failure,
                    "Multicall returned incomplete results; provider marked as failure"
                );
                self.rpc_pool.report_failure(&provider);
                last_error = Some(anyhow!(
                    "Multicall returned incomplete results ({} of {}, partial_failure={}) in attempt {}/{}",
                    results.len(),
                    total_calls,
                    partial_failure,
                    attempt,
                    max_attempts
                ));
                if attempt == max_attempts {
                    break;
                } else {
                    continue;
                }
            }

            let all_results = results;

            let multicall_duration = start_time.elapsed();
            self.rpc_pool.report_success(&provider, multicall_duration);

            let (
                v3_updates,
                v3_balances,
                v2_updates,
                curve_updates,
                balancer_updates,
                successful_calls,
                v3_partial_fail_counts,
                v2_partial_fail_counts,
            ) = self
                .parse_multicall_results(&all_results, &call_index_map, &provider)
                .await?;

            let fetch_duration = start_time.elapsed();

            info!("Unified state fetch completed: {}/{} successful calls in {:?} ({} V3, {} V2, {} Curve, {} Balancer)", 
              successful_calls, total_calls, fetch_duration,
              v3_updates.len(), v2_updates.len(), curve_updates.len(), balancer_updates.len());

            // Use BlockNumberCache if available, otherwise fallback to provider
            let head_block = if let Some(ref cache) = self.block_number_cache {
                cache.get_current_block().await.unwrap_or_else(|_| {
                    warn!("BlockNumberCache failed, falling back to provider");
                    0u64
                })
            } else {
                0u64
            };
            
            let head_block = if head_block == 0 {
                provider
                    .get_block_number()
                    .await
                    .unwrap_or_default()
                    .as_u64()
            } else {
                head_block
            };

            return Ok(UnifiedStateUpdate {
                v3_updates,
                v3_balances,
                v2_updates,
                curve_updates,
                balancer_updates,
                fetch_duration,
                total_calls,
                successful_calls,
                block_number: head_block,
                v3_requested: v3_pools_to_refresh,
                v2_requested: v2_pools_to_refresh,
                curve_requested: curve_pools_to_refresh,
                balancer_requested: balancer_pools_to_refresh,
                v3_partial_fail_counts,
                v2_partial_fail_counts,
            });
        }

        // Fallback to direct fetch for small pool sets
        if let Some(fallback_update) = self
            .direct_fetch_fallback(
                hot_manager,
                &v3_pools_to_refresh,
                &v2_pools_to_refresh,
                &curve_pools_to_refresh,
                &balancer_pools_to_refresh,
            )
            .await
        {
            return Ok(fallback_update);
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow!(
                "Unified state fetch failed after {} attempts with empty multicall results",
                max_attempts
            )
        }))
    }

    async fn direct_fetch_fallback(
        &mut self,
        hot_manager: &HotPoolManager,
        v3_pools: &[Address],
        v2_pools: &[Address],
        curve_pools: &[Address],
        balancer_pools: &[Address],
    ) -> Option<UnifiedStateUpdate> {
        let subset_v3 = Self::take_subset(v3_pools, DIRECT_FALLBACK_V3_LIMIT);
        let subset_v2 = Self::take_subset(v2_pools, DIRECT_FALLBACK_V2_LIMIT);
        let subset_curve = Self::take_subset(curve_pools, DIRECT_FALLBACK_CURVE_LIMIT);
        let subset_balancer = Self::take_subset(balancer_pools, DIRECT_FALLBACK_BALANCER_LIMIT);

        if subset_v3.is_empty()
            && subset_v2.is_empty()
            && subset_curve.is_empty()
            && subset_balancer.is_empty()
        {
            return None;
        }

        let fallback_start = Instant::now();
        let (provider, _permit) = match self.rpc_pool.acquire(RpcRole::State).await {
            Ok(provider) => provider,
            Err(err) => {
                warn!(
                    error = ?err,
                    "Failed to acquire provider for direct fallback fetch"
                );
                return None;
            }
        };

        let mut v3_updates = HashMap::new();
        if !subset_v3.is_empty() {
            v3_updates = self.direct_fetch_v3(&provider, &subset_v3).await;
        }

        let mut v2_updates = HashMap::new();
        if !subset_v2.is_empty() {
            v2_updates = self.direct_fetch_v2(&provider, &subset_v2).await;
        }

        let mut curve_updates = HashMap::new();
        if !subset_curve.is_empty() {
            curve_updates = self
                .direct_fetch_curve(&provider, hot_manager, &subset_curve)
                .await;
        }

        let mut balancer_updates = HashMap::new();
        if !subset_balancer.is_empty() {
            balancer_updates = self
                .direct_fetch_balancer(&provider, hot_manager, &subset_balancer)
                .await;
        }

        let total_updates =
            v3_updates.len() + v2_updates.len() + curve_updates.len() + balancer_updates.len();

        if total_updates == 0 {
            warn!("Direct fallback fetch produced zero updates");
            return None;
        }

        let fetch_duration = fallback_start.elapsed();

        // Use BlockNumberCache if available, otherwise fallback to provider
        let block_number = if let Some(ref cache) = self.block_number_cache {
            cache.get_current_block().await.unwrap_or_else(|_| {
                warn!("BlockNumberCache failed, falling back to provider");
                0u64
            })
        } else {
            0u64
        };
        
        let block_number = if block_number == 0 {
            provider
                .get_block_number()
                .await
                .unwrap_or_default()
                .as_u64()
        } else {
            block_number
        };

        let total_calls =
            subset_v3.len() + subset_v2.len() + subset_curve.len() + subset_balancer.len();
        let successful_calls =
            v3_updates.len() + v2_updates.len() + curve_updates.len() + balancer_updates.len();

        info!(
            "Direct RPC fallback succeeded: {} V3, {} V2, {} Curve, {} Balancer",
            v3_updates.len(),
            v2_updates.len(),
            curve_updates.len(),
            balancer_updates.len()
        );

        Some(UnifiedStateUpdate {
            v3_updates,
            v3_balances: HashMap::new(),
            v2_updates,
            curve_updates,
            balancer_updates,
            fetch_duration,
            total_calls,
            successful_calls,
            block_number,
            v3_requested: subset_v3,
            v2_requested: subset_v2,
            curve_requested: subset_curve,
            balancer_requested: subset_balancer,
            v3_partial_fail_counts: HashMap::new(),
            v2_partial_fail_counts: HashMap::new(),
        })
    }

    async fn direct_fetch_v3(
        &mut self,
        provider: &Arc<Provider<Http>>,
        pools: &[Address],
    ) -> HashMap<Address, V3PoolState> {
        if pools.is_empty() {
            return HashMap::new();
        }

        // Use multicall batch instead of individual calls
        let mut calls = Vec::new();
        let mut call_index_map = Vec::new();

        for &pool_addr in pools {
            let pool_contract = UniswapV3Pool::new(pool_addr, Arc::clone(provider));
            calls.push(Call {
                target: pool_addr,
                call_data: pool_contract.slot_0().calldata().unwrap(),
            });
            call_index_map.push(("v3_slot0", pool_addr));

            calls.push(Call {
                target: pool_addr,
                call_data: pool_contract.liquidity().calldata().unwrap(),
            });
            call_index_map.push(("v3_liquidity", pool_addr));
        }

        let mc = Multicall::new(Arc::clone(provider), self.multicall_address, self.batch_size);
        let results = match mc.run(calls, None).await {
            Ok(res) => res,
            Err(e) => {
                warn!(error = ?e, "Direct fallback V3 multicall failed, returning empty");
                return HashMap::new();
            }
        };

        // Parse results using existing parser
        let (v3_updates, _, _, _, _, _, _, _) = match self
            .parse_multicall_results(&results, &call_index_map, provider)
            .await
        {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(error = ?e, "Failed to parse direct fallback V3 results");
                return HashMap::new();
            }
        };

        v3_updates
    }

    async fn direct_fetch_v2(
        &mut self,
        provider: &Arc<Provider<Http>>,
        pools: &[Address],
    ) -> HashMap<Address, (U256, U256)> {
        if pools.is_empty() {
            return HashMap::new();
        }

        // Use multicall batch instead of individual calls
        let mut calls = Vec::new();
        let mut call_index_map = Vec::new();

        for &pool_addr in pools {
            let pair_contract = IUniswapV2Pair::new(pool_addr, Arc::clone(provider));
            calls.push(Call {
                target: pool_addr,
                call_data: pair_contract.get_reserves().calldata().unwrap(),
            });
            call_index_map.push(("v2_reserves", pool_addr));
        }

        let mc = Multicall::new(Arc::clone(provider), self.multicall_address, self.batch_size);
        let results = match mc.run(calls, None).await {
            Ok(res) => res,
            Err(e) => {
                warn!(error = ?e, "Direct fallback V2 multicall failed, returning empty");
                return HashMap::new();
            }
        };

        // Parse results using existing parser
        let (_, _, v2_updates, _, _, _, _, _) = match self
            .parse_multicall_results(&results, &call_index_map, provider)
            .await
        {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(error = ?e, "Failed to parse direct fallback V2 results");
                return HashMap::new();
            }
        };

        v2_updates
    }

    async fn direct_fetch_curve(
        &mut self,
        provider: &Arc<Provider<Http>>,
        hot_manager: &HotPoolManager,
        pools: &[Address],
    ) -> HashMap<Address, CurveState> {
        if pools.is_empty() {
            return HashMap::new();
        }

        // Use multicall batch instead of individual calls
        let mut calls = Vec::new();
        let mut call_index_map = Vec::new();

        for &pool_addr in pools {
            let snapshot = match hot_manager.curve_hot_pools.get(&pool_addr) {
                Some(entry) => entry.tokens.len(),
                None => {
                    debug!(pool = ?pool_addr, "Direct fallback skipped Curve pool without snapshot");
                    continue;
                }
            };

            let curve_contract = ICurvePool::new(pool_addr, Arc::clone(provider));
            let num_tokens = snapshot.min(2); // Support up to 2 tokens

            for i in 0..num_tokens {
                calls.push(Call {
                    target: pool_addr,
                    call_data: curve_contract.balances(i.into()).calldata().unwrap(),
                });
                call_index_map.push(("curve_balance", pool_addr));
            }

            calls.push(Call {
                target: pool_addr,
                call_data: curve_contract.a().calldata().unwrap(),
            });
            call_index_map.push(("curve_a", pool_addr));

            calls.push(Call {
                target: pool_addr,
                call_data: curve_contract.fee().calldata().unwrap(),
            });
            call_index_map.push(("curve_fee", pool_addr));
        }

        if calls.is_empty() {
            return HashMap::new();
        }

        let mc = Multicall::new(Arc::clone(provider), self.multicall_address, self.batch_size);
        let results = match mc.run(calls, None).await {
            Ok(res) => res,
            Err(e) => {
                warn!(error = ?e, "Direct fallback Curve multicall failed, returning empty");
                return HashMap::new();
            }
        };

        // Parse results using existing parser
        let (_, _, _, curve_updates, _, _, _, _) = match self
            .parse_multicall_results(&results, &call_index_map, provider)
            .await
        {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(error = ?e, "Failed to parse direct fallback Curve results");
                return HashMap::new();
            }
        };

        curve_updates
    }

    async fn direct_fetch_balancer(
        &mut self,
        provider: &Arc<Provider<Http>>,
        hot_manager: &HotPoolManager,
        pools: &[Address],
    ) -> HashMap<Address, BalancerState> {
        if pools.is_empty() {
            return HashMap::new();
        }

        let vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
            .parse()
            .unwrap();

        // Use multicall batch instead of individual calls
        let mut calls = Vec::new();
        let mut call_index_map = Vec::new();

        for &pool_addr in pools {
            let snapshot = match hot_manager.balancer_hot_pools.get(&pool_addr) {
                Some(entry) => entry.pool_id,
                None => {
                    debug!(pool = ?pool_addr, "Direct fallback skipped Balancer pool without snapshot");
                    continue;
                }
            };

            let vault = IBalancerV2Vault::new(vault_addr, Arc::clone(provider));
            let pool = IWeightedPool::new(pool_addr, Arc::clone(provider));
            let pool_id_bytes = snapshot;

            calls.push(Call {
                target: vault_addr,
                call_data: vault.get_pool_tokens(pool_id_bytes).calldata().unwrap(),
            });
            call_index_map.push(("balancer_tokens", pool_addr));

            calls.push(Call {
                target: pool_addr,
                call_data: pool.get_swap_fee_percentage().calldata().unwrap(),
            });
            call_index_map.push(("balancer_fee", pool_addr));
        }

        if calls.is_empty() {
            return HashMap::new();
        }

        let mc = Multicall::new(Arc::clone(provider), self.multicall_address, self.batch_size);
        let results = match mc.run(calls, None).await {
            Ok(res) => res,
            Err(e) => {
                warn!(error = ?e, "Direct fallback Balancer multicall failed, returning empty");
                return HashMap::new();
            }
        };

        // Parse results using existing parser
        let (_, _, _, _, balancer_updates, _, _, _) = match self
            .parse_multicall_results(&results, &call_index_map, provider)
            .await
        {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(error = ?e, "Failed to parse direct fallback Balancer results");
                return HashMap::new();
            }
        };

        balancer_updates
    }

    fn take_subset(pools: &[Address], limit: usize) -> Vec<Address> {
        pools.iter().copied().take(limit).collect()
    }

    /// Parse multicall results into structured updates using DataPipeline
    async fn parse_multicall_results(
        &mut self,
        results: &[ethers::types::Bytes],
        call_index_map: &[(&str, Address)],
        provider: &Arc<Provider<Http>>,
    ) -> Result<(
        HashMap<Address, V3PoolState>,
        HashMap<Address, (U256, U256)>,
        HashMap<Address, (U256, U256)>,
        HashMap<Address, CurveState>,
        HashMap<Address, BalancerState>,
        usize,
        HashMap<Address, u8>,
        HashMap<Address, u8>,
    )> {
        let mut v3_updates = HashMap::new();
        let mut v2_updates = HashMap::new();
        let mut curve_updates: HashMap<Address, CurveState> = HashMap::new();
        let mut balancer_updates: HashMap<Address, BalancerState> = HashMap::new();
        let mut successful_calls = 0;
        let mut v3_partial_fail_counts: HashMap<Address, u8> = HashMap::new();
        let mut v2_partial_fail_counts: HashMap<Address, u8> = HashMap::new();

        debug!(
            "Parsing {} multicall results through data pipeline",
            results.len()
        );

        // Create dummy contracts for ABI decoding
        let dummy_v3 = UniswapV3Pool::new(Address::zero(), Arc::clone(provider));
        let slot0_fn = dummy_v3.abi().function("slot0")?;
        let liquidity_fn = dummy_v3.abi().function("liquidity")?;

        let dummy_v2 = IUniswapV2Pair::new(Address::zero(), Arc::clone(provider));
        let reserves_fn = dummy_v2.abi().function("getReserves")?;

        let dummy_curve = ICurvePool::new(Address::zero(), Arc::clone(provider));
        let curve_balances_fn = dummy_curve.abi().function("balances")?;
        let curve_a_fn = dummy_curve.abi().function("A")?;
        let curve_fee_fn = dummy_curve.abi().function("fee")?;

        let vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
            .parse()
            .unwrap();
        let dummy_vault = IBalancerV2Vault::new(vault_addr, Arc::clone(provider));
        let balancer_tokens_fn = dummy_vault.abi().function("getPoolTokens")?;

        let dummy_weighted_pool = IWeightedPool::new(Address::zero(), Arc::clone(provider));
        let balancer_fee_fn = dummy_weighted_pool.abi().function("getSwapFeePercentage")?;

        let dummy_erc20 = Erc20::new(Address::zero(), Arc::clone(provider));
        let balance_fn = dummy_erc20.abi().function("balanceOf")?;

        // Track V3 pool states being built
        let mut v3_partial_states: HashMap<Address, PartialV3State> = HashMap::new();
        let mut v3_balance_partial: HashMap<Address, (Option<U256>, Option<U256>)> = HashMap::new();

        // Track Curve pool states being built
        let mut curve_partial_states: HashMap<Address, PartialCurveState> = HashMap::new();

        // Track Balancer pool states being built
        let mut balancer_partial_states: HashMap<Address, PartialBalancerState> = HashMap::new();

        for (i, (call_type, pool_addr)) in call_index_map.iter().enumerate() {
            if i >= results.len() {
                warn!(
                    call_index = i,
                    total_results = results.len(),
                    "Multicall result index out of bounds"
                );
                continue;
            }

            let result_data = &results[i];
            if result_data.0.is_empty() {
                warn!(pool_addr = ?pool_addr, "Empty result for call: {}", call_type);
                match *call_type {
                    "v3_slot0" | "v3_liquidity" => {
                        let c = v3_partial_fail_counts.entry(*pool_addr).or_insert(0);
                        *c = c.saturating_add(1);
                    }
                    "v2_reserves" => {
                        let c = v2_partial_fail_counts.entry(*pool_addr).or_insert(0);
                        *c = c.saturating_add(1);
                    }
                    _ => {}
                }
                continue;
            }

            match *call_type {
                "v3_slot0" => {
                    match slot0_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let (Some(sqrt_price), Some(tick_token)) = (
                                decoded.get(0).and_then(|t| t.clone().into_uint()),
                                decoded.get(1).and_then(|t| t.clone().into_int()),
                            ) {
                                let tick_i32 = if tick_token.bit(23) {
                                    let mask = U256::from(0xFFFFFF);
                                    -((((!tick_token & mask) + 1) & mask).as_u32() as i32)
                                } else {
                                    tick_token.as_u32() as i32
                                };

                                let partial = v3_partial_states
                                    .entry(*pool_addr)
                                    .or_insert(PartialV3State::default());
                                partial.sqrt_price_x96 = Some(sqrt_price);
                                partial.tick = Some(tick_i32 as i64);
                                successful_calls += 1;
                            }
                        }
                        Err(e) => {
                            warn!(pool_addr = ?pool_addr, error = ?e, "V3 slot0 decoding failed");
                            let c = v3_partial_fail_counts.entry(*pool_addr).or_insert(0);
                            *c = c.saturating_add(1);
                        }
                    }
                }
                "v3_liquidity" => {
                    match liquidity_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let Some(liquidity_u256) = decoded[0].clone().into_uint() {
                                if let Some(liquidity) = liquidity_u256.try_into().ok() {
                                    if liquidity > 0 {
                                        let partial = v3_partial_states
                                            .entry(*pool_addr)
                                            .or_insert(PartialV3State::default());
                                        partial.liquidity = Some(liquidity);
                                        successful_calls += 1;
                                    } else {
                                        warn!(pool_addr = ?pool_addr, "V3 pool has zero liquidity");
                                    }
                                } else {
                                    warn!(pool_addr = ?pool_addr, "V3 liquidity conversion failed");
                                }
                            }
                        }
                        Err(e) => {
                            error!(pool_addr = ?pool_addr, error = ?e, "Failed to decode V3 liquidity output");
                            let c = v3_partial_fail_counts.entry(*pool_addr).or_insert(0);
                            *c = c.saturating_add(1);
                        }
                    }
                }
                "v3_balance0" => match balance_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(balance_uint) =
                            decoded.get(0).and_then(|t| t.clone().into_uint())
                        {
                            let partial =
                                v3_balance_partial.entry(*pool_addr).or_insert((None, None));
                            partial.0 = Some(balance_uint);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(
                            pool_addr = ?pool_addr,
                            error = ?e,
                            "Failed to decode V3 token0 balance"
                        );
                    }
                },
                "v3_balance1" => match balance_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(balance_uint) =
                            decoded.get(0).and_then(|t| t.clone().into_uint())
                        {
                            let partial =
                                v3_balance_partial.entry(*pool_addr).or_insert((None, None));
                            partial.1 = Some(balance_uint);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(
                            pool_addr = ?pool_addr,
                            error = ?e,
                            "Failed to decode V3 token1 balance"
                        );
                    }
                },
                "v2_reserves" => {
                    match reserves_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let (Some(reserve0), Some(reserve1)) = (
                                decoded[0].clone().into_uint(),
                                decoded[1].clone().into_uint(),
                            ) {
                                let reserve0_hex = format!("0x{:x}", reserve0);
                                let reserve1_hex = format!("0x{:x}", reserve1);

                                let raw_input = (
                                    reserve0_hex,
                                    reserve1_hex,
                                    18u8,
                                    18u8,
                                    DataSource::MulticalV2,
                                );

                                match self.pipeline.process::<NormalizedV2Reserves>(
                                    raw_input,
                                    DataSource::MulticalV2,
                                ) {
                                    Ok(conversion_result) => {
                                        if conversion_result.is_valid {
                                            v2_updates.insert(*pool_addr, (reserve0, reserve1));
                                            successful_calls += 1;
                                        } else {
                                            warn!(pool_addr = ?pool_addr, errors = ?conversion_result.validation_errors, "V2 reserves validation failed");
                                        }
                                    }
                                    Err(e) => {
                                        error!(pool_addr = ?pool_addr, error = ?e, "V2 reserves pipeline processing failed");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!(pool_addr = ?pool_addr, error = ?e, "Failed to decode V2 reserves output");
                            let c = v2_partial_fail_counts.entry(*pool_addr).or_insert(0);
                            *c = c.saturating_add(1);
                        }
                    }
                }
                "curve_balance" => match curve_balances_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(balance) = decoded[0].clone().into_uint() {
                            let partial = curve_partial_states
                                .entry(*pool_addr)
                                .or_insert(PartialCurveState::default());
                            partial.balances.push(balance);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(pool_addr = ?pool_addr, error = ?e, "Curve balance decoding failed");
                    }
                },
                "curve_a" => match curve_a_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(a) = decoded[0].clone().into_uint() {
                            let partial = curve_partial_states
                                .entry(*pool_addr)
                                .or_insert(PartialCurveState::default());
                            partial.a = Some(a);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(pool_addr = ?pool_addr, error = ?e, "Curve A decoding failed");
                    }
                },
                "curve_fee" => match curve_fee_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(fee) = decoded[0].clone().into_uint() {
                            let partial = curve_partial_states
                                .entry(*pool_addr)
                                .or_insert(PartialCurveState::default());
                            partial.fee = Some(fee);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(pool_addr = ?pool_addr, error = ?e, "Curve fee decoding failed");
                    }
                },
                "balancer_tokens" => {
                    match balancer_tokens_fn.decode_output(&result_data.0) {
                        Ok(decoded) => {
                            if let (Some(tokens_array), Some(balances_array)) = (
                                decoded.get(0).and_then(|t| t.clone().into_array()),
                                decoded.get(1).and_then(|t| t.clone().into_array()),
                            ) {
                                let tokens: Vec<Address> = tokens_array
                                    .iter()
                                    .filter_map(|t| t.clone().into_address())
                                    .collect();
                                let balances: Vec<U256> = balances_array
                                    .iter()
                                    .filter_map(|t| t.clone().into_uint())
                                    .collect();

                                let partial = balancer_partial_states
                                    .entry(*pool_addr)
                                    .or_insert(PartialBalancerState::default());
                                partial.tokens = tokens;
                                partial.balances = balances;
                                successful_calls += 1;
                            }
                        }
                        Err(e) => {
                            warn!(pool_addr = ?pool_addr, error = ?e, "Balancer tokens decoding failed");
                        }
                    }
                }
                "balancer_fee" => match balancer_fee_fn.decode_output(&result_data.0) {
                    Ok(decoded) => {
                        if let Some(fee) = decoded[0].clone().into_uint() {
                            let partial = balancer_partial_states
                                .entry(*pool_addr)
                                .or_insert(PartialBalancerState::default());
                            partial.swap_fee = Some(fee);
                            successful_calls += 1;
                        }
                    }
                    Err(e) => {
                        warn!(pool_addr = ?pool_addr, error = ?e, "Balancer fee decoding failed");
                    }
                },
                _ => {
                    warn!("Unknown call type: {}", call_type);
                }
            }
        }

        // Finalize V3 states (only if we have both slot0 and liquidity)
        for (pool_addr, partial) in v3_partial_states {
            if let (Some(sqrt_price), Some(tick), Some(liquidity)) =
                (partial.sqrt_price_x96, partial.tick, partial.liquidity)
            {
                v3_updates.insert(
                    pool_addr,
                    V3PoolState {
                        sqrt_price_x96: sqrt_price,
                        tick,
                        liquidity,
                    },
                );
            }
        }

        // Finalize Curve states
        for (pool_addr, partial) in curve_partial_states {
            if let Some(a) = partial.a {
                if !partial.balances.is_empty() {
                    let fee = partial.fee.unwrap_or_else(|| {
                        U256::from(4000000000000000u64) // 0.04% default
                    });

                    curve_updates.insert(
                        pool_addr,
                        CurveState {
                            balances: partial.balances,
                            a,
                            fee,
                        },
                    );
                }
            }
        }

        // Finalize Balancer states
        for (pool_addr, partial) in balancer_partial_states {
            if let Some(swap_fee) = partial.swap_fee {
                if !partial.tokens.is_empty() && !partial.balances.is_empty() {
                    balancer_updates.insert(
                        pool_addr,
                        BalancerState {
                            tokens: partial.tokens,
                            balances: partial.balances,
                            weights: partial.weights,
                            swap_fee,
                        },
                    );
                }
            }
        }

        info!("Multicall parsing completed: {} V3, {} V2, {} Curve, {} Balancer updates, {} successful calls",
              v3_updates.len(), v2_updates.len(), curve_updates.len(), balancer_updates.len(), successful_calls);

        let mut v3_balances = HashMap::new();
        for (addr, (bal0, bal1)) in v3_balance_partial {
            v3_balances.insert(addr, (bal0.unwrap_or_default(), bal1.unwrap_or_default()));
        }

        Ok((
            v3_updates,
            v3_balances,
            v2_updates,
            curve_updates,
            balancer_updates,
            successful_calls,
            v3_partial_fail_counts,
            v2_partial_fail_counts,
        ))
    }

    /// Apply state updates to hot pool manager
    pub async fn apply_updates(
        &self,
        hot_manager: &HotPoolManager,
        updates: &UnifiedStateUpdate,
    ) -> Result<()> {
        let now = Instant::now();

        // Update V3 pools
        for (pool_addr, new_state) in &updates.v3_updates {
            if let Some(mut entry) = hot_manager.v3_hot_pools.get_mut(pool_addr) {
                if new_state.sqrt_price_x96.is_zero() || new_state.liquidity == 0 {
                    if entry.state.sqrt_price_x96.is_zero() || entry.state.liquidity == 0 {
                        warn!("Pool {} still has zero state after refresh - marking as Corrupt", pool_addr);
                        entry.state_quality = StateQuality::Corrupt;
                        entry.last_updated = now;
                    }
                    continue;
                }

                let original_token0 = entry.token0;
                let original_token1 = entry.token1;
                let original_fee = entry.fee;
                let original_dex = entry.dex;
                let (balance0, balance1) = updates
                    .v3_balances
                    .get(pool_addr)
                    .cloned()
                    .unwrap_or((entry.token0_balance, entry.token1_balance));

                let quality = self
                    .classify_v3_quality_async(
                        entry.token0,
                        entry.token1,
                        new_state,
                        entry.last_updated,
                    )
                    .await;

                let mut new_partial = entry.partial_fail_count;
                if let Some(add) = updates.v3_partial_fail_counts.get(pool_addr) {
                    new_partial = new_partial.saturating_add(*add);
                }

                *entry = V3PoolSnapshot {
                    address: entry.address,
                    token0: original_token0,
                    token1: original_token1,
                    fee: original_fee,
                    state: new_state.clone(),
                    last_updated: now,
                    weight: entry.weight,
                    dex: original_dex,
                    state_quality: quality,
                    partial_fail_count: new_partial,
                    approximate: new_partial >= 2,
                    slot0_updated_at: Some(now),
                    liquidity_updated_at: Some(now),
                    max_safe_amount_usd: entry.max_safe_amount_usd,
                    price_deviation_bps: entry.price_deviation_bps,
                    last_validated_at: Some(now),
                    token0_balance: balance0,
                    token1_balance: balance1,
                    last_tvl_estimate: entry.last_tvl_estimate,
                    tick_bitmap: entry.tick_bitmap.clone(),
                    ticks: entry.ticks.clone(),
                    last_tick_refresh: entry.last_tick_refresh,
                    last_refreshed_tick: entry.last_refreshed_tick,
                    block_number: updates.block_number,
                    price_std: entry.price_std,
                    liquidity_change_rate: entry.liquidity_change_rate,
                };
            }
        }

        // Update V2 pools
        for (pool_addr, (reserve0, reserve1)) in &updates.v2_updates {
            if let Some(mut entry) = hot_manager.v2_hot_pools.get_mut(pool_addr) {
                if reserve0.is_zero() || reserve1.is_zero() {
                    if entry.reserve0.is_zero() && entry.reserve1.is_zero() {
                        warn!("V2 pool {} still has zero reserves after refresh - marking as Corrupt", pool_addr);
                        entry.state_quality = StateQuality::Corrupt;
                        entry.last_updated = now;
                    }
                    continue;
                }

                entry.reserve0 = *reserve0;
                entry.reserve1 = *reserve1;
                entry.last_updated = now;
                entry.state_quality = self
                    .classify_v2_quality_async(
                        entry.token0,
                        entry.token1,
                        *reserve0,
                        *reserve1,
                        entry.last_updated,
                    )
                    .await;
                if let Some(add) = updates.v2_partial_fail_counts.get(pool_addr) {
                    entry.partial_fail_count = entry.partial_fail_count.saturating_add(*add);
                    entry.approximate = entry.partial_fail_count >= 2;
                } else {
                    entry.partial_fail_count = 0;
                    entry.approximate = false;
                }
                entry.reserves_updated_at = Some(now);
                entry.last_validated_at = Some(now);
                entry.block_number = updates.block_number;
            }
        }

        // Update Curve pools
        for (pool_addr, new_state) in &updates.curve_updates {
            if let Some(mut entry) = hot_manager.curve_hot_pools.get_mut(pool_addr) {
                entry.balances = new_state.balances.clone();
                entry.a = new_state.a;
                entry.fee = new_state.fee;
                entry.last_updated = now;
                entry.balances_updated_at = Some(now);
                entry.params_updated_at = Some(now);
                entry.block_number = updates.block_number;
                entry.state_quality = self
                    .classify_curve_quality_async(
                        &entry.tokens,
                        &entry.balances,
                        entry.last_updated,
                    )
                    .await;
            }
        }

        // Update Balancer pools
        for (pool_addr, new_state) in &updates.balancer_updates {
            if let Some(mut entry) = hot_manager.balancer_hot_pools.get_mut(pool_addr) {
                entry.balances = new_state.balances.clone();
                entry.weights = new_state.weights.clone();
                entry.swap_fee = new_state.swap_fee;
                entry.last_updated = now;
                entry.balances_updated_at = Some(now);
                entry.weights_updated_at = Some(now);
                entry.block_number = updates.block_number;
                entry.state_quality = self
                    .classify_balancer_quality_async(
                        &entry.tokens,
                        &entry.balances,
                        &entry.weights,
                        entry.last_updated,
                    )
                    .await;
            }
        }

        Ok(())
    }

    async fn classify_v3_quality_async(
        &self,
        _token0: Address,
        _token1: Address,
        state: &V3PoolState,
        last_updated_prev: Instant,
    ) -> StateQuality {
        let age = last_updated_prev.elapsed().as_secs();
        if state.liquidity == 0 {
            return StateQuality::Corrupt;
        }
        if age > self.settings.data_quality.state_max_age_secs {
            return StateQuality::Stale;
        }
        StateQuality::Fresh
    }

    async fn classify_v2_quality_async(
        &self,
        _token0: Address,
        _token1: Address,
        r0: U256,
        r1: U256,
        last_updated_prev: Instant,
    ) -> StateQuality {
        let age = last_updated_prev.elapsed().as_secs();
        if r0.is_zero() || r1.is_zero() {
            return StateQuality::Corrupt;
        }
        if age > self.settings.data_quality.state_max_age_secs {
            return StateQuality::Stale;
        }
        StateQuality::Fresh
    }

    async fn classify_curve_quality_async(
        &self,
        _tokens: &[Address],
        balances: &[U256],
        last_updated_prev: Instant,
    ) -> StateQuality {
        let age = last_updated_prev.elapsed().as_secs();
        if balances.is_empty() || balances.iter().any(|b| b.is_zero()) {
            return StateQuality::Corrupt;
        }
        if age > self.settings.data_quality.state_max_age_secs {
            return StateQuality::Stale;
        }
        StateQuality::Fresh
    }

    async fn classify_balancer_quality_async(
        &self,
        _tokens: &[Address],
        balances: &[U256],
        _weights: &[U256],
        last_updated_prev: Instant,
    ) -> StateQuality {
        let age = last_updated_prev.elapsed().as_secs();
        if balances.is_empty() || balances.iter().any(|b| b.is_zero()) {
            return StateQuality::Corrupt;
        }
        if age > self.settings.data_quality.state_max_age_secs {
            return StateQuality::Stale;
        }
        StateQuality::Fresh
    }
}

#[derive(Debug, Default)]
struct PartialV3State {
    sqrt_price_x96: Option<U256>,
    tick: Option<i64>,
    liquidity: Option<u128>,
}

#[derive(Debug, Default)]
struct PartialCurveState {
    balances: Vec<U256>,
    a: Option<U256>,
    fee: Option<U256>,
}

#[derive(Debug, Default)]
struct PartialBalancerState {
    tokens: Vec<Address>,
    balances: Vec<U256>,
    weights: Vec<U256>,
    swap_fee: Option<U256>,
}
