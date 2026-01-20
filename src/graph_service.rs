//! # Graph Service
//!
//! The `GraphService` maintains a weighted liquidity graph representing the topology
//! of all validated pools. It calculates liquidity weights and provides graph queries.
//!
//! ## Overview
//!
//! The graph service:
//! - Maintains in-memory graph weights (liquidity-weighted edges)
//! - Calculates weights based on pool TVL and token prices
//! - Provides fast lookups for pool weights
//! - Persists weights to PostgreSQL for durability
//!
//! ## Weight Calculation
//!
//! Pool weights represent the liquidity depth of each pool, calculated as:
//! ```
//! weight = sqrt(reserve0 * price0 * reserve1 * price1)
//! ```
//!
//! This provides a geometric mean that balances both reserves proportionally.
//!
//! ## Thread Safety
//!
//! The graph service uses `DashMap` for lock-free concurrent reads, allowing
//! thousands of concurrent queries without blocking.

use crate::pools::{Pool, UniswapV2Pool, UniswapV3Pool};
use crate::price_feeds::PriceFeed;
use crate::rpc_pool::RpcPool;
use crate::multicall::{self, Multicall, Call};
use crate::contracts::{Erc20, IUniswapV2Pair, UniswapV3Pool as UniswapV3PoolContract};
use crate::v3_math::{u256_to_f64_lossy, V3PoolState};
// Removed JIT Fetcher dependency - using direct method instead
use crate::flight_recorder::FlightRecorder;
use crate::{record_phase_start, record_phase_end, record_cache_event};
use std::sync::Arc;
use std::convert::TryInto;
use anyhow::Result;
use ethers::prelude::{Address, Provider, Http, Middleware};
use ethers::types::U256;
use std::collections::HashMap;
use dashmap::DashMap;
use crate::database::{self, DbPool};
use crate::hot_pool_manager::HotPoolManager;
#[cfg(feature = "redis")]
use crate::redis_manager::{RedisManager, CachedPoolState};
use crate::background_price_updater::{SharedPriceCache, PriceSource};
use tracing::{info, warn, debug};
use std::time::{Instant, Duration};

/// Maximum reasonable weight in USD to prevent calculation errors.
/// Any weight above this threshold is considered invalid.
const MAX_REASONABLE_WEIGHT_USD: f64 = 10_000_000_000_000.0; // $10T

/// Maintains the weighted liquidity graph for topology queries.
///
/// The graph service provides:
/// - Fast weight lookups via in-memory `DashMap`
/// - Weight calculation based on pool TVL
/// - Persistence to PostgreSQL
///
/// # Generic Parameters
///
/// - `M`: Middleware type for blockchain interactions
///
/// # Example
///
/// ```rust,no_run
/// use mig_topology_sdk::graph_service::GraphService;
///
/// let graph_service = GraphService::new(
///     rpc_pool,
///     price_feed,
///     db_pool,
///     multicall_address,
/// ).await?;
///
/// // Get weight for a pool
/// if let Some(weight) = graph_service.get_weight(&pool_address) {
///     println!("Pool weight: ${}", weight);
/// }
/// ```
pub struct GraphService<M: Middleware> {
    rpc_pool: Arc<RpcPool>,
    price_feed: Arc<PriceFeed<M>>,
    db_pool: DbPool,
    weights: Arc<DashMap<Address, f64>>,
    multicall_address: Address, // ✅ Dirección de Multicall desde Settings
    settings: Arc<crate::settings::Settings>, // ✅ P1 OPTIMIZATION: Store settings for parallel fetching
    flight_recorder: Option<Arc<FlightRecorder>>, // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    hot_pool_manager: Option<Arc<HotPoolManager>>, // ✅ HOT POOL MANAGER: Optional top-K pool cache with adaptive refresh
    block_number_cache: Option<Arc<crate::block_number_cache::BlockNumberCache>>, // ✅ BlockNumberCache integration
    shared_price_cache: Option<Arc<SharedPriceCache>>, // ✅ SHARED PRICE CACHE: Optional shared cache for anchor tokens and pool fallback
    #[cfg(feature = "redis")]
    redis_manager: Option<Arc<tokio::sync::Mutex<RedisManager>>>, // ✅ REDIS: Optional Redis cache for pool states
}

impl<M: Middleware + 'static> GraphService<M> {
    /*
    pub fn new_mock() -> Self {
        let settings = crate::settings::Settings::new().unwrap();
        let rpc_pool = Arc::new(crate::rpc_pool::RpcPool::new(Arc::new(settings.clone())).unwrap());
        let cache_manager = Arc::new(crate::cache::CacheManager::new());
        let (provider, _) = futures::executor::block_on(rpc_pool.get_next_provider()).unwrap();
        let price_feed = Arc::new(crate::price_feeds::PriceFeed::new(
            provider,
            Default::default(),
            10,
            settings.contracts.factories.multicall.parse().unwrap(),
            100,
            cache_manager,
            vec![],
        ));
        let weights = Arc::new(DashMap::new());
        std::env::set_var("DATABASE_URL", "postgresql://user:pass@host/db");
        let db_pool = futures::executor::block_on(database::connect()).unwrap();
        Self { rpc_pool, price_feed, db_pool, weights }
    }
    */

    /// Creates a new graph service instance.
    ///
    /// # Parameters
    ///
    /// - `rpc_pool`: RPC provider pool for blockchain queries
    /// - `price_feed`: Price feed service for token prices
    /// - `db_pool`: PostgreSQL connection pool
    /// - `multicall_address`: Address of the multicall contract for batch queries
    ///
    /// # Returns
    ///
    /// A new `GraphService` instance with weights loaded from the database.
    ///
    /// # Errors
    ///
    /// Returns an error if database connection fails or weight loading fails.
    pub async fn new(
        rpc_pool: Arc<RpcPool>, 
        price_feed: Arc<PriceFeed<M>>, 
        db_pool: DbPool,
        multicall_address: Address,
        settings: Arc<crate::settings::Settings>,
    ) -> Result<Self> {
        let weights = Arc::new(DashMap::new());
        let initial_weights = database::load_all_graph_weights(&db_pool).await?;
        for (address, weight) in initial_weights {
            weights.insert(address, weight);
        }
        info!("GraphService initialized with {} weights from database.", weights.len());
        Ok(Self { 
            rpc_pool, 
            price_feed, 
            db_pool, 
            weights,
            multicall_address, // ✅ Guardar dirección
            settings, // ✅ P1 OPTIMIZATION: Store settings for parallel fetching
            flight_recorder: None,
            hot_pool_manager: None,
            block_number_cache: None,
            shared_price_cache: None,
            #[cfg(feature = "redis")]
            redis_manager: None,
        })
    }

    /// Set block number cache to avoid individual get_block_number() calls
    pub fn with_block_number_cache(mut self, block_number_cache: Arc<crate::block_number_cache::BlockNumberCache>) -> Self {
        self.block_number_cache = Some(block_number_cache);
        self
    }
    
    /// Set flight recorder for instrumentation
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder);
        self
    }
    
    /// Set Hot Pool Manager for top-K pool caching with adaptive refresh rates
    pub fn with_hot_pool_manager(mut self, hot_pool_manager: Arc<HotPoolManager>) -> Self {
        self.hot_pool_manager = Some(hot_pool_manager);
        self
    }
    
    /// Set Shared Price Cache for anchor tokens and pool fallback support
    pub fn with_shared_price_cache(mut self, shared_price_cache: Arc<SharedPriceCache>) -> Self {
        self.shared_price_cache = Some(shared_price_cache);
        self
    }
    
    /// Set Redis Manager for distributed caching (optional, requires redis feature)
    #[cfg(feature = "redis")]
    pub fn with_redis(mut self, redis_manager: Arc<tokio::sync::Mutex<RedisManager>>) -> Self {
        self.redis_manager = Some(redis_manager);
        self
    }

    /// Gets the current weight for a pool.
    ///
    /// # Parameters
    ///
    /// - `pool_address`: Address of the pool to query
    ///
    /// # Returns
    ///
    /// The pool's weight in USD, or `None` if the pool is not in the graph.
    ///
    /// # Performance
    ///
    /// This is a lock-free read operation with O(1) average case complexity.
    pub fn get_weight(&self, pool_address: &Address) -> Option<f64> {
        self.weights.get(pool_address).map(|w| *w)
    }

    /// ✅ INCREMENTAL: Calculates and updates weights for specific pools only (fast path)
    ///
    /// This method is optimized for discovery cycles where only a few pools are discovered.
    /// It only processes the specified pool addresses instead of all active pools.
    ///
    /// # Performance
    ///
    /// - For 10 pools: ~50ms
    /// - For 100 pools: ~200ms
    /// - For 1,000 pools: ~400ms
    ///
    /// # Parameters
    ///
    /// - `pool_addresses`: Vector of pool addresses to update weights for
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if weight calculation completes successfully.
    ///
    /// # Errors
    ///
    /// Returns an error if database queries fail or RPC calls fail.
    pub async fn calculate_and_update_weights_for_pools(&self, pool_addresses: &[Address]) -> Result<()> {
        // ✅ FLIGHT RECORDER: Registrar inicio de graph_updates (incremental)
        let graph_update_start = Instant::now();
        
        // ✅ HOT POOL REFRESH: Get hot pool addresses from Hot Pool Manager
        let mut all_pool_addresses = pool_addresses.to_vec();
        let mut hot_pool_count = 0;
        
        if let Some(ref hot_pool_manager) = self.hot_pool_manager {
            // Collect all hot pool addresses from all pool types
            let mut hot_pools = Vec::new();
            for entry in hot_pool_manager.v3_hot_pools.iter() {
                hot_pools.push(*entry.key());
            }
            for entry in hot_pool_manager.v2_hot_pools.iter() {
                hot_pools.push(*entry.key());
            }
            for entry in hot_pool_manager.curve_hot_pools.iter() {
                hot_pools.push(*entry.key());
            }
            for entry in hot_pool_manager.balancer_hot_pools.iter() {
                hot_pools.push(*entry.key());
            }
            
            hot_pool_count = hot_pools.len();
            
            // Add hot pools that are not already in the recent list
            for hot_addr in hot_pools {
                if !all_pool_addresses.contains(&hot_addr) {
                    all_pool_addresses.push(hot_addr);
                }
            }
        }
        
        if all_pool_addresses.is_empty() {
            return Ok(());
        }
        
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "graph_updates", serde_json::json!({
                "mode": "incremental",
                "recent_pools": pool_addresses.len(),
                "hot_pools_added": hot_pool_count,
                "total_pools": all_pool_addresses.len()
            }));
        }
        
        info!(
            "📊 Calculating weights for {} pools (incremental update: {} recent + {} hot)",
            all_pool_addresses.len(),
            pool_addresses.len(),
            hot_pool_count
        );
        
        // 1. Load pools from database (includes both recent and hot)
        let pools = database::load_pools_by_addresses(&self.db_pool, &all_pool_addresses).await?;
        if pools.is_empty() {
            info!("No pools found for provided addresses");
            return Ok(());
        }
        
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        let current_block = if let Some(ref cache) = self.block_number_cache {
            cache.get_current_block().await.unwrap_or_else(|_| {
                warn!("BlockNumberCache failed, falling back to provider");
                0u64
            })
        } else {
            0u64
        };
        
        let current_block = if current_block == 0 {
            provider.get_block_number().await?.as_u64()
        } else {
            current_block
        };
        
        // 2. Fetch pool states (with Redis cache - hot pools should HIT)
        let fetch_start = Instant::now();
        let pools_with_state = self.fetch_pool_states(pools, provider.clone()).await?;
        let fetch_duration_ms = fetch_start.elapsed().as_millis() as u64;
        
        info!(
            "✅ Fetched {} pool states in {}ms (Redis cache utilized for hot pools)",
            pools_with_state.len(),
            fetch_duration_ms
        );
        
        // 3. Collect unique tokens and fetch prices/decimals in batch
        let mut unique_tokens = std::collections::HashSet::new();
        for pool in &pools_with_state {
            match pool {
                Pool::UniswapV2(p) => {
                    unique_tokens.insert(p.token0);
                    unique_tokens.insert(p.token1);
                },
                Pool::UniswapV3(p) => {
                    unique_tokens.insert(p.token0);
                    unique_tokens.insert(p.token1);
                },
                Pool::BalancerWeighted(p) => {
                    for token in &p.tokens {
                        unique_tokens.insert(*token);
                    }
                },
                Pool::CurveStableSwap(p) => {
                    for token in &p.tokens {
                        unique_tokens.insert(*token);
                    }
                },
            }
        }
        
        let tokens_vec: Vec<Address> = unique_tokens.into_iter().collect();
        
        // ✅ P1 OPTIMIZATION: Parallel price fetching - divide tokens into chunks and fetch in parallel
        // ✅ FIX: Use longer timeout (500ms per chunk) and SharedPriceCache for pool fallback
        let prices_map = if self.settings.performance.parallel_price_fetching_enabled 
            && tokens_vec.len() > self.settings.performance.price_fetch_chunk_size {
            let chunk_size = self.settings.performance.price_fetch_chunk_size;
            let chunks: Vec<_> = tokens_vec.chunks(chunk_size).collect();
            info!("✅ [P1] Parallel price fetching: {} tokens split into {} chunks (chunk_size={})", 
                  tokens_vec.len(), chunks.len(), chunk_size);
            
            // ✅ FIX: Use SharedPriceCache if available for anchor tokens and pool fallback
            // Clone the Arc outside the loop to share across all tasks
            let shared_cache_arc = self.shared_price_cache.clone();
            
            // Fetch all chunks in parallel with longer timeout and SharedPriceCache
            use futures::future::join_all;
            let mut tasks = Vec::new();
            for chunk in chunks {
                let chunk_tokens = chunk.to_vec();
                let price_feed_clone = Arc::clone(&self.price_feed);
                let shared_cache_for_task = shared_cache_arc.clone();
                tasks.push(async move {
                    // ✅ FIX: Use timeout of 500ms per chunk (more reasonable for Chainlink + pool fallback)
                    price_feed_clone.get_usd_prices_batch_with_chainlink_timeout_and_cache(
                        &chunk_tokens, 
                        None, 
                        Duration::from_millis(500),
                        shared_cache_for_task.as_ref().map(|c| c.as_ref())
                    ).await
                });
            }
            
            let chunk_results = join_all(tasks).await;
            
            // ✅ FIX: Merge all results, including partial successes
            let mut merged_prices = HashMap::new();
            let mut successful_chunks = 0;
            let mut failed_chunks = 0;
            let mut total_prices_from_chunks = 0;
            
            for (idx, chunk_result) in chunk_results.iter().enumerate() {
                match chunk_result {
                    Ok(prices) => {
                        if !prices.is_empty() {
                            merged_prices.extend(prices.clone());
                            total_prices_from_chunks += prices.len();
                            successful_chunks += 1;
                        } else {
                            warn!("⚠️ [P1] Price fetch chunk {} returned empty prices (may be timeout or all tokens failed)", idx);
                            failed_chunks += 1;
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [P1] Price fetch chunk {} failed completely: {}", idx, e);
                        failed_chunks += 1;
                    }
                }
            }
            
            info!("✅ [P1] Parallel price fetch completed: {} prices from {} successful chunks ({} failed chunks)", 
                  merged_prices.len(), successful_chunks, failed_chunks);
            
            // ✅ FIX: Warn if we got very few prices
            if merged_prices.len() < tokens_vec.len() / 10 {
                warn!("⚠️ [P1] Only got {} prices for {} tokens ({}%). This may cause most pools to have weight=0.", 
                      merged_prices.len(), tokens_vec.len(), (merged_prices.len() * 100) / tokens_vec.len().max(1));
            }
            
            merged_prices
        } else {
            // Sequential fetching (fallback or when parallel is disabled)
            // ✅ FIX: Use SharedPriceCache and longer timeout for sequential fetch too
            let shared_cache_ref = self.shared_price_cache.as_ref().map(|c| c.as_ref());
            match self.price_feed.get_usd_prices_batch_with_chainlink_timeout_and_cache(
                &tokens_vec, 
                None, 
                Duration::from_millis(2000), // 2s timeout for sequential (can wait longer)
                shared_cache_ref
            ).await {
                Ok(mut prices) => {
                    if prices.is_empty() {
                        warn!("⚠️ Sequential price fetch returned empty prices for {} tokens", tokens_vec.len());
                    } else {
                        // ✅ EMERGENCY PRICE REPAIR: Attempt to recover missing prices
                        let missing_tokens: Vec<Address> = tokens_vec.iter()
                            .filter(|t| !prices.contains_key(t) || prices.get(t).map(|&p| p <= 0.0).unwrap_or(true))
                            .copied()
                            .collect();
                        
                        if !missing_tokens.is_empty() {
                            // Limit to max 20 tokens for emergency repair
                            let repair_tokens: Vec<Address> = missing_tokens.iter().take(20).copied().collect();
                            info!("🔧 [Emergency Repair] Attempting to recover {} missing prices (limited to {} tokens)", 
                                  missing_tokens.len(), repair_tokens.len());
                            
                            match self.price_feed
                                .get_usd_prices_batch_with_chainlink_timeout(
                                    &repair_tokens,
                                    None,
                                    Duration::from_millis(1500)
                                )
                                .await
                            {
                                Ok(repair_prices) => {
                                    let recovered = repair_prices.values().filter(|&&p| p > 0.0).count();
                                    prices.extend(repair_prices);
                                    info!("✅ [Emergency Repair] Recovered {} prices (total: {})", recovered, prices.len());
                                    
                                    // Update SharedPriceCache if available
                                    if let Some(cache) = &self.shared_price_cache {
                                        let valid_repair: HashMap<Address, f64> = prices.iter()
                                            .filter(|(_, &p)| p > 0.0)
                                            .map(|(k, v)| (*k, *v))
                                            .collect();
                                        if !valid_repair.is_empty() {
                                            cache.update_batch(valid_repair, PriceSource::Chainlink);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ [Emergency Repair] Failed: {}", e);
                                }
                            }
                        }
                    }
                    prices
                },
                Err(e) => {
                    warn!("Failed to get prices batch: {}. Continuing with empty prices.", e);
                    HashMap::new()
                }
            }
        };
        
        let decimals_map = match self.get_decimals(&tokens_vec).await {
            Ok(decimals) => decimals,
            Err(e) => {
                warn!("Failed to get decimals batch: {}. Continuing with defaults.", e);
                HashMap::new()
            }
        };
        
        // 4. Calculate weights and update in-memory graph and database
        // ✅ P1 OPTIMIZATION: Collect weights first, then batch update database
        let mut total_updated = 0;
        let mut weights_for_hot_pool = HashMap::new();
        let mut weights_to_update: Vec<(Address, f64, u64)> = Vec::new();
        
        for pool in pools_with_state {
            let pool_address = pool.address();
            match self.calculate_liquidity_usd_with_cache(&pool, &prices_map, &decimals_map).await {
                Ok(liquidity_usd) => {
                    let final_weight = if liquidity_usd > MAX_REASONABLE_WEIGHT_USD {
                        warn!("⚠️ Pool {} has extreme weight: ${:.2}. Filtering to 0.", pool_address, liquidity_usd);
                        0.0
                    } else {
                        liquidity_usd
                    };
                    
                    self.weights.insert(pool_address, final_weight);
                    weights_for_hot_pool.insert(pool_address, final_weight);
                    weights_to_update.push((pool_address, final_weight, current_block));
                    total_updated += 1;
                },
                Err(e) => {
                    warn!("Failed to calculate liquidity for pool {}: {}", pool_address, e);
                }
            }
        }
        
        // ✅ P1 OPTIMIZATION: Batch update database (much faster than individual updates)
        if !weights_to_update.is_empty() {
            let weights_to_update_clone = weights_to_update.clone();
            match database::batch_upsert_graph_weights(&self.db_pool, &weights_to_update_clone).await {
                Ok(_) => {
                    debug!("✅ [P1] Batch updated {} graph weights in database", weights_to_update_clone.len());
                }
                Err(e) => {
                    warn!("⚠️ [P1] Batch update failed, falling back to individual updates: {}", e);
                    // Fallback to individual updates if batch fails
                    for (pool_address, weight, block) in weights_to_update_clone {
                        let pool_addr_hex = format!("{:#x}", pool_address);
                        if let Err(e) = database::upsert_graph_weight(&self.db_pool, &pool_addr_hex, weight, block).await {
                            warn!("Failed to upsert graph weight for pool {}: {}", pool_address, e);
                        }
                    }
                }
            }
        }
        
        // ✅ HOT POOL MANAGER: Update weights if available
        if let Some(ref hot_pool_manager) = self.hot_pool_manager {
            if !weights_for_hot_pool.is_empty() {
                hot_pool_manager.update_weights(weights_for_hot_pool).await;
            }
        }
        
        // ✅ FLIGHT RECORDER: Registrar fin de graph_updates (incremental)
        let state_staleness_ms = graph_update_start.elapsed().as_millis() as u64;
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "graph_updates", graph_update_start, serde_json::json!({
                "mode": "incremental",
                "recent_pools": pool_addresses.len(),
                "hot_pools_refreshed": hot_pool_count,
                "total_pools_processed": all_pool_addresses.len(),
                "pools_updated": total_updated,
                "fetch_duration_ms": fetch_duration_ms,
                "state_staleness_ms": state_staleness_ms,
                // ✅ P1 OPTIMIZATION: Batch DB update metrics
                "batch_db_update": true,
                "weights_batch_size": weights_to_update.len(),
                // ✅ P1 OPTIMIZATION: Parallel price fetching metrics
                "parallel_price_fetch_enabled": self.settings.performance.parallel_price_fetching_enabled,
                "price_fetch_chunk_size": self.settings.performance.price_fetch_chunk_size
            }));
        }
        
        info!(
            "✅ Incremental weight update completed: {} pools updated ({} recent + {} hot)",
            total_updated,
            pool_addresses.len(),
            hot_pool_count
        );
        Ok(())
    }

    /// Calculates and updates weights for all active pools (full refresh - expensive).
    ///
    /// This method:
    /// 1. Loads all active pools from the database
    /// 2. Fetches current pool states via multicall
    /// 3. Calculates weights based on reserves and prices
    /// 4. Updates in-memory graph and database
    ///
    /// # Performance
    ///
    /// - For 1,000 pools: ~400ms
    /// - For 2,000 pools: ~800ms
    /// - For 26,000 pools: ~56s
    ///
    /// Performance is dominated by price fetching (60%) and weight calculation (30%).
    ///
    /// # Usage
    ///
    /// Use this method for:
    /// - Initial weight calculation
    /// - Periodic full refresh (e.g., every 20 minutes)
    /// - Recovery after errors
    ///
    /// For discovery cycles, use `calculate_and_update_weights_for_pools()` instead.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if weight calculation completes successfully.
    ///
    /// # Errors
    ///
    /// Returns an error if database queries fail or RPC calls fail.
    pub async fn calculate_and_update_all_weights(&self) -> Result<()> {
        // ✅ FLIGHT RECORDER: Registrar inicio de graph_updates
        let graph_update_start = Instant::now();
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "graph_updates", serde_json::json!({}));
        }
        
        info!("Starting to calculate and update all pool weights...");
        // ✅ IMPORTANTE: Solo calcular weights para pools activos
        // Los pools activos son los que tienen liquidez y están vivos (marcados por check_pools_activity)
        // Calcular weights para pools inactivos sería inútil porque no tienen liquidez
        let pools = database::load_active_pools(&self.db_pool).await?;
        if pools.is_empty() {
            info!("No active pools found to update weights.");
            return Ok(());
        }
        info!("📊 Processing {} active pools for weight calculation (rotating through all active pools)", pools.len());

        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        // Use BlockNumberCache if available, otherwise fallback to provider
        let current_block = if let Some(ref cache) = self.block_number_cache {
            cache.get_current_block().await.unwrap_or_else(|_| {
                warn!("BlockNumberCache failed, falling back to provider");
                0u64
            })
        } else {
            0u64
        };
        
        let current_block = if current_block == 0 {
            provider.get_block_number().await?.as_u64()
        } else {
            current_block
        };

        // 📊 Track weight distribution for analysis (acumulado para todos los batches)
        let mut weight_distribution: Vec<f64> = Vec::new();
        let mut zero_weight_count = 0;
        let mut low_weight_count = 0;  // < 1.0
        let mut medium_weight_count = 0; // 1.0 - 10.0
        let mut high_weight_count = 0; // > 10.0
        
        // ✅ HOT POOL MANAGER: Accumulate calculated weights for Hot Pool Manager update
        let mut calculated_weights_for_hot_pool: HashMap<Address, f64> = HashMap::new();
        
        // Procesar todos los pools en un solo batch (2000 pools)
        // El multicall internamente hace chunks según su batch_size (500 del Config)
        let mut total_processed = 0;
        let mut total_updated = 0;
        
        info!("📦 Processing {} pools in a single batch", pools.len());
        
        // ✅ REFACTOR: Use direct method for all pools (no JIT Fetcher)
        // Process all pools in a single batch using direct multicall
        let pools_with_state_vec = self.fetch_pool_states(pools, provider.clone()).await?;
        total_processed += pools_with_state_vec.len();

        // ✅ OPTIMIZACIÓN: Recolectar todos los tokens únicos y obtener precios/decimals en batch
        let mut unique_tokens = std::collections::HashSet::new();
        for pool in &pools_with_state_vec {
            match pool {
                Pool::UniswapV2(p) => {
                    unique_tokens.insert(p.token0);
                    unique_tokens.insert(p.token1);
                },
                Pool::UniswapV3(p) => {
                    unique_tokens.insert(p.token0);
                    unique_tokens.insert(p.token1);
                },
                Pool::BalancerWeighted(p) => {
                    for token in &p.tokens {
                        unique_tokens.insert(*token);
                    }
                },
                Pool::CurveStableSwap(p) => {
                    for token in &p.tokens {
                        unique_tokens.insert(*token);
                    }
                },
            }
        }
        
        let tokens_vec: Vec<Address> = unique_tokens.into_iter().collect();
        info!("📊 Pre-loading prices and decimals for {} unique tokens...", tokens_vec.len());
        
        // ✅ P1 OPTIMIZATION: Parallel price fetching - divide tokens into chunks and fetch in parallel
        // ✅ FIX: Use longer timeout (2000ms per chunk for full refresh) and SharedPriceCache for pool fallback
        let mut prices_map = if self.settings.performance.parallel_price_fetching_enabled 
            && tokens_vec.len() > self.settings.performance.price_fetch_chunk_size {
            let chunk_size = self.settings.performance.price_fetch_chunk_size;
            let chunks: Vec<_> = tokens_vec.chunks(chunk_size).collect();
            info!("✅ [P1] Parallel price fetching: {} tokens split into {} chunks (chunk_size={})", 
                  tokens_vec.len(), chunks.len(), chunk_size);
            
            // ✅ FIX: Use SharedPriceCache if available for anchor tokens and pool fallback
            // Clone the Arc outside the loop to share across all tasks
            let shared_cache_arc = self.shared_price_cache.clone();
            
            // Fetch all chunks in parallel with longer timeout (full refresh can wait longer)
            use futures::future::join_all;
            let mut tasks = Vec::new();
            for chunk in chunks {
                let chunk_tokens = chunk.to_vec();
                let price_feed_clone = Arc::clone(&self.price_feed);
                let shared_cache_for_task = shared_cache_arc.clone();
                tasks.push(async move {
                    // ✅ FIX: Use timeout of 2000ms per chunk for full refresh (background operation, can wait longer)
                    price_feed_clone.get_usd_prices_batch_with_chainlink_timeout_and_cache(
                        &chunk_tokens, 
                        None, 
                        Duration::from_millis(2000),
                        shared_cache_for_task.as_ref().map(|c| c.as_ref())
                    ).await
                });
            }
            
            let chunk_results = join_all(tasks).await;
            
            // ✅ FIX: Merge all results, including partial successes
            let mut merged_prices = HashMap::new();
            let mut successful_chunks = 0;
            let mut failed_chunks = 0;
            let mut total_prices_from_chunks = 0;
            
            for (idx, chunk_result) in chunk_results.iter().enumerate() {
                match chunk_result {
                    Ok(prices) => {
                        if !prices.is_empty() {
                            merged_prices.extend(prices.clone());
                            total_prices_from_chunks += prices.len();
                            successful_chunks += 1;
                        } else {
                            warn!("⚠️ [P1] Price fetch chunk {} returned empty prices (may be timeout or all tokens failed)", idx);
                            failed_chunks += 1;
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [P1] Price fetch chunk {} failed completely: {}", idx, e);
                        failed_chunks += 1;
                    }
                }
            }
            
            info!("✅ [P1] Parallel price fetch completed: {} prices from {} successful chunks ({} failed chunks)", 
                  merged_prices.len(), successful_chunks, failed_chunks);
            
            // ✅ FIX: Warn if we got very few prices
            if merged_prices.len() < tokens_vec.len() / 10 {
                warn!("⚠️ [P1] Only got {} prices for {} tokens ({}%). This may cause most pools to have weight=0.", 
                      merged_prices.len(), tokens_vec.len(), (merged_prices.len() * 100) / tokens_vec.len().max(1));
            }
            
            // ✅ EMERGENCY PRICE REPAIR: Attempt to recover missing prices with longer timeout
            if merged_prices.len() < tokens_vec.len() / 2 {
                let missing_tokens: Vec<Address> = tokens_vec.iter()
                    .filter(|t| !merged_prices.contains_key(t) || merged_prices.get(t).map(|&p| p <= 0.0).unwrap_or(true))
                    .copied()
                    .collect();
                
                if !missing_tokens.is_empty() {
                    // Limit to max 20 tokens for emergency repair to avoid timeout
                    let repair_tokens: Vec<Address> = missing_tokens.iter().take(20).copied().collect();
                    info!("🔧 [Emergency Repair] Attempting to recover {} missing prices (limited to {} tokens)", 
                          missing_tokens.len(), repair_tokens.len());
                    
                    match self.price_feed
                        .get_usd_prices_batch_with_chainlink_timeout(
                            &repair_tokens,
                            None,
                            Duration::from_millis(1500)
                        )
                        .await
                    {
                        Ok(repair_prices) => {
                            let recovered = repair_prices.values().filter(|&&p| p > 0.0).count();
                            merged_prices.extend(repair_prices);
                            info!("✅ [Emergency Repair] Recovered {} prices (total: {})", recovered, merged_prices.len());
                            
                            // Update SharedPriceCache if available
                            if let Some(cache) = &self.shared_price_cache {
                                let valid_repair: HashMap<Address, f64> = merged_prices.iter()
                                    .filter(|(_, &p)| p > 0.0)
                                    .map(|(k, v)| (*k, *v))
                                    .collect();
                                if !valid_repair.is_empty() {
                                    cache.update_batch(valid_repair, PriceSource::Chainlink);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️ [Emergency Repair] Failed: {}", e);
                        }
                    }
                }
            }
            
            merged_prices
        } else {
            // Sequential fetching (fallback or when parallel is disabled)
            // ✅ FIX: Use SharedPriceCache and longer timeout for sequential fetch too
            let shared_cache_ref = self.shared_price_cache.as_ref().map(|c| c.as_ref());
            match self.price_feed.get_usd_prices_batch_with_chainlink_timeout_and_cache(
                &tokens_vec, 
                None, 
                Duration::from_millis(3000), // 3s timeout for sequential full refresh (can wait even longer)
                shared_cache_ref
            ).await {
                Ok(mut prices) => {
                    if prices.is_empty() {
                        warn!("⚠️ Sequential price fetch returned empty prices for {} tokens", tokens_vec.len());
                    } else {
                        // ✅ EMERGENCY PRICE REPAIR: Attempt to recover missing prices
                        let missing_tokens: Vec<Address> = tokens_vec.iter()
                            .filter(|t| !prices.contains_key(t) || prices.get(t).map(|&p| p <= 0.0).unwrap_or(true))
                            .copied()
                            .collect();
                        
                        if !missing_tokens.is_empty() {
                            // Limit to max 20 tokens for emergency repair
                            let repair_tokens: Vec<Address> = missing_tokens.iter().take(20).copied().collect();
                            info!("🔧 [Emergency Repair] Attempting to recover {} missing prices (limited to {} tokens)", 
                                  missing_tokens.len(), repair_tokens.len());
                            
                            match self.price_feed
                                .get_usd_prices_batch_with_chainlink_timeout(
                                    &repair_tokens,
                                    None,
                                    Duration::from_millis(1500)
                                )
                                .await
                            {
                                Ok(repair_prices) => {
                                    let recovered = repair_prices.values().filter(|&&p| p > 0.0).count();
                                    prices.extend(repair_prices);
                                    info!("✅ [Emergency Repair] Recovered {} prices (total: {})", recovered, prices.len());
                                    
                                    // Update SharedPriceCache if available
                                    if let Some(cache) = &self.shared_price_cache {
                                        let valid_repair: HashMap<Address, f64> = prices.iter()
                                            .filter(|(_, &p)| p > 0.0)
                                            .map(|(k, v)| (*k, *v))
                                            .collect();
                                        if !valid_repair.is_empty() {
                                            cache.update_batch(valid_repair, PriceSource::Chainlink);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ [Emergency Repair] Failed: {}", e);
                                }
                            }
                        }
                    }
                    prices
                },
                Err(e) => {
                    warn!("Failed to get prices batch: {}. Continuing with empty prices.", e);
                    HashMap::new()
                }
            }
        };
        
        // Obtener todos los decimals en batch (multicall)
        let decimals_map = match self.get_decimals(&tokens_vec).await {
            Ok(decimals) => {
                if decimals.is_empty() {
                    warn!("⚠️  get_decimals returned empty map for {} tokens. This may cause all weights to be 0.", tokens_vec.len());
                }
                decimals
            },
            Err(e) => {
                warn!("Failed to get decimals batch: {}. Falling back to sequential.", e);
                HashMap::new()
            }
        };
        
        info!("✅ Pre-loaded {} prices and {} decimals (out of {} unique tokens)", prices_map.len(), decimals_map.len(), tokens_vec.len());
        
        // ✅ VALIDATION: Check if we have enough prices before calculating weights
        if prices_map.len() < tokens_vec.len() / 10 {
            warn!("⚠️  Only loaded {} prices for {} tokens. Most pools will have weight = 0.", prices_map.len(), tokens_vec.len());
            
            // ✅ EMERGENCY REPAIR: Try to recover missing prices before continuing
            let missing_tokens: Vec<Address> = tokens_vec.iter()
                .filter(|t| !prices_map.contains_key(t) || prices_map.get(t).map(|&p| p <= 0.0).unwrap_or(true))
                .copied()
                .collect();
            
            if !missing_tokens.is_empty() && missing_tokens.len() < tokens_vec.len() / 2 {
                // Only attempt repair if missing less than 50%
                let repair_tokens: Vec<Address> = missing_tokens.iter().take(20).copied().collect();
                info!("🔧 [Emergency Repair] Attempting to recover {} missing prices before weight calculation", repair_tokens.len());
                
                match self.price_feed
                    .get_usd_prices_batch_with_chainlink_timeout(
                        &repair_tokens,
                        None,
                        Duration::from_millis(1500)
                    )
                    .await
                {
                    Ok(repair_prices) => {
                        let recovered = repair_prices.values().filter(|&&p| p > 0.0).count();
                        for (token, price) in repair_prices {
                            if price > 0.0 {
                                prices_map.insert(token, price);
                            }
                        }
                        info!("✅ [Emergency Repair] Recovered {} prices before weight calculation (total: {})", recovered, prices_map.len());
                        
                        // Update SharedPriceCache if available
                        if let Some(cache) = &self.shared_price_cache {
                            let valid_repair: HashMap<Address, f64> = prices_map.iter()
                                .filter(|(_, &p)| p > 0.0)
                                .map(|(k, v)| (*k, *v))
                                .collect();
                            if !valid_repair.is_empty() {
                                cache.update_batch(valid_repair, PriceSource::Chainlink);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [Emergency Repair] Failed before weight calculation: {}", e);
                    }
                }
            }
        }

        // Procesar todos los pools usando los datos pre-cargados
        // ✅ P1 OPTIMIZATION: Collect weights first, then batch update database
        let mut filtered_extreme_count = 0;
        let mut weights_to_update: Vec<(Address, f64, u64)> = Vec::new();
        
        for pool in pools_with_state_vec {
            let pool_address = pool.address();
            match self.calculate_liquidity_usd_with_cache(&pool, &prices_map, &decimals_map).await {
                Ok(liquidity_usd) => {
                    // 🔍 FILTRO: Detectar y manejar errores de cálculo
                    let final_weight = if liquidity_usd > MAX_REASONABLE_WEIGHT_USD {
                        filtered_extreme_count += 1;
                        warn!(
                            "⚠️ Pool {} has extreme weight: ${:.2} (likely calculation error). Filtering to 0.",
                            pool_address, liquidity_usd
                        );
                        0.0 // Filtrar valores extremos (error de cálculo)
                    } else {
                        liquidity_usd
                    };
                    
                    self.weights.insert(pool_address, final_weight);
                    weight_distribution.push(final_weight);
                    weights_to_update.push((pool_address, final_weight, current_block));
                    total_updated += 1;
                    
                    // ✅ HOT POOL MANAGER: Add weight to hot pool manager map
                    calculated_weights_for_hot_pool.insert(pool_address, final_weight);
                    
                    // Categorize weights
                    if final_weight == 0.0 {
                        zero_weight_count += 1;
                    } else if final_weight < 1.0 {
                        low_weight_count += 1;
                    } else if final_weight < 10.0 {
                        medium_weight_count += 1;
                    } else {
                        high_weight_count += 1;
                    }
                },
                Err(e) => {
                    warn!("Failed to calculate liquidity for pool {}: {}", pool_address, e);
                }
            }
        }
        
        // ✅ P1 OPTIMIZATION: Batch update database (much faster than individual updates)
        if !weights_to_update.is_empty() {
            let weights_to_update_clone = weights_to_update.clone();
            match database::batch_upsert_graph_weights(&self.db_pool, &weights_to_update_clone).await {
                Ok(_) => {
                    info!("✅ [P1] Batch updated {} graph weights in database", weights_to_update_clone.len());
                }
                Err(e) => {
                    warn!("⚠️ [P1] Batch update failed, falling back to individual updates: {}", e);
                    // Fallback to individual updates if batch fails
                    for (pool_address, weight, block) in weights_to_update_clone {
                        let pool_addr_hex = format!("{:#x}", pool_address);
                        if let Err(e) = database::upsert_graph_weight(&self.db_pool, &pool_addr_hex, weight, block).await {
                            warn!("Failed to upsert graph weight for pool {}: {}", pool_address, e);
                        }
                    }
                }
            }
        }
        
        if filtered_extreme_count > 0 {
            warn!("🚨 Filtered {} pools with extreme weights (calculation errors)", filtered_extreme_count);
        }
        
        info!("✅ Processed {} pools in a single batch, updated {} weights", total_processed, total_updated);
        
        // ✅ FLIGHT RECORDER: Registrar fin de graph_updates (full refresh) con métricas P0/P1
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "graph_updates", graph_update_start, serde_json::json!({
                "mode": "full_refresh",
                "total_pools_processed": total_processed,
                "pools_updated": total_updated,
                "zero_weight_count": zero_weight_count,
                "low_weight_count": low_weight_count,
                "medium_weight_count": medium_weight_count,
                "high_weight_count": high_weight_count,
                "filtered_extreme_count": filtered_extreme_count,
                // ✅ P1 OPTIMIZATION: Batch DB update metrics
                "batch_db_update": true,
                "weights_batch_size": weights_to_update.len(),
                // ✅ P1 OPTIMIZATION: Parallel price fetching metrics
                "parallel_price_fetch_enabled": self.settings.performance.parallel_price_fetching_enabled,
                "price_fetch_chunk_size": self.settings.performance.price_fetch_chunk_size,
                "unique_tokens": tokens_vec.len(),
                "prices_loaded": prices_map.len()
            }));
        }
        
        // 📊 Log weight distribution statistics
        if !weight_distribution.is_empty() {
            weight_distribution.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let total = weight_distribution.len();
            let p25 = weight_distribution[total / 4];
            let p50 = weight_distribution[total / 2];
            let p75 = weight_distribution[total * 3 / 4];
            let p90 = weight_distribution[(total * 9) / 10];
            let p95 = weight_distribution[(total * 95) / 100];
            let max = weight_distribution[total - 1];
            let sum: f64 = weight_distribution.iter().sum();
            let avg = sum / total as f64;
            
            info!("📊 WEIGHT DISTRIBUTION ANALYSIS:");
            info!("   Total pools: {}", total);
            info!("   Zero weight: {} ({:.1}%)", zero_weight_count, 100.0 * zero_weight_count as f64 / total as f64);
            info!("   Low weight (<1.0): {} ({:.1}%)", low_weight_count, 100.0 * low_weight_count as f64 / total as f64);
            info!("   Medium weight (1.0-10.0): {} ({:.1}%)", medium_weight_count, 100.0 * medium_weight_count as f64 / total as f64);
            info!("   High weight (>10.0): {} ({:.1}%)", high_weight_count, 100.0 * high_weight_count as f64 / total as f64);
            info!("   Average: {:.2}", avg);
            info!("   Percentiles - P25: {:.2}, P50: {:.2}, P75: {:.2}, P90: {:.2}, P95: {:.2}", p25, p50, p75, p90, p95);
            info!("   Max: {:.2}", max);
            info!("   💡 Suggested threshold (P25): {:.2}", p25);
        }
        
        info!("Finished calculating and updating weights for {} pools.", self.weights.len());
        
        // ✅ HOT POOL MANAGER: Update Hot Pool Manager weights if available
        if let Some(ref hot_pool_manager) = self.hot_pool_manager {
            if !calculated_weights_for_hot_pool.is_empty() {
                hot_pool_manager.update_weights(calculated_weights_for_hot_pool.clone()).await;
                info!("✅ Hot Pool Manager weights updated ({} pools)", calculated_weights_for_hot_pool.len());
            }
        }
        
        // ✅ FLIGHT RECORDER: Registrar fin de graph_updates con state_staleness_ms
        // Nota: state_staleness_ms es aproximado usando duration_ms (diferencia entre inicio y fin del cálculo)
        let state_staleness_ms = graph_update_start.elapsed().as_millis() as u64;
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "graph_updates", graph_update_start, serde_json::json!({
                "pools_processed": total_processed,
                "pools_updated": total_updated,
                "state_staleness_ms": state_staleness_ms,
                "hot_pool_manager_updated": calculated_weights_for_hot_pool.len()
            }));
        }
        
        Ok(())
    }

    // Removed pool_to_metadata and fresh_states_to_pools - no longer using JIT Fetcher

    /// Fetch pool states from on-chain (with Redis cache support)
    /// 
    /// This method is public to allow external code (like Hot Pool Manager population)
    /// to fetch pool states while benefiting from Redis cache and optimized multicall.
    pub async fn fetch_pool_states(&self, pools: Vec<Pool>, provider: Arc<Provider<Http>>) -> Result<Vec<Pool>> {
        let pools_len = pools.len(); // Save length before move
        // ✅ REDIS: Try to fetch from Redis cache first
        #[cfg(feature = "redis")]
        if let Some(ref redis_mgr) = self.redis_manager {
            info!("🔍 Redis cache enabled, checking {} pools", pools_len);
            let mut redis_guard = redis_mgr.lock().await;
            let mut cached_pools = Vec::new();
            let mut pools_to_fetch = Vec::new();
            
            for pool in pools {
                let addr = pool.address();
                let addr_str = format!("{:#x}", addr);
                match redis_guard.get_pool_state(&addr_str).await {
                    Ok(Some(cached_state)) => {
                        // Convert cached state back to Pool
                        match self.cached_state_to_pool(&cached_state, &pool) {
                            Ok(converted_pool) => {
                                cached_pools.push(converted_pool);
                                // ✅ FLIGHT RECORDER: Registrar cache hit
                                if let Some(ref recorder) = self.flight_recorder {
                                    let current_block = if let Some(ref cache) = self.block_number_cache {
                                        cache.get_current_block().await.ok()
                                    } else {
                                        None
                                    };
                                    record_cache_event!(recorder, "pool_state", "hit", &addr_str, current_block);
                                }
                                continue;
                            }
                            Err(e) => {
                                // ✅ FIX: Si falla la conversión, registrar como miss y continuar
                                debug!("⚠️ Failed to convert cached state for pool {}: {}, treating as miss", addr_str, e);
                                if let Some(ref recorder) = self.flight_recorder {
                                    let current_block = if let Some(ref cache) = self.block_number_cache {
                                        cache.get_current_block().await.ok()
                                    } else {
                                        None
                                    };
                                    record_cache_event!(recorder, "pool_state", "miss", &addr_str, current_block);
                                }
                                // Continue to fetch this pool
                            }
                        }
                    }
                    Ok(None) => {
                        // ✅ FLIGHT RECORDER: Registrar cache miss (no encontrado)
                        if let Some(ref recorder) = self.flight_recorder {
                            let current_block = if let Some(ref cache) = self.block_number_cache {
                                cache.get_current_block().await.ok()
                            } else {
                                None
                            };
                            record_cache_event!(recorder.clone(), "pool_state", "miss", &addr_str, current_block);
                        }
                    }
                    Err(e) => {
                        debug!("Redis cache miss for pool {}: {}", addr_str, e);
                        // ✅ FLIGHT RECORDER: Registrar cache miss (error)
                        if let Some(ref recorder) = self.flight_recorder {
                            let current_block = if let Some(ref cache) = self.block_number_cache {
                                cache.get_current_block().await.ok()
                            } else {
                                None
                            };
                            record_cache_event!(recorder, "pool_state", "miss", &addr_str, current_block);
                        }
                    }
                }
                pools_to_fetch.push(pool);
            }
            
            let cache_hits = cached_pools.len();
            let cache_misses = pools_len - cache_hits;
            if cache_hits > 0 {
                info!("✅ Redis cache hit: {}/{} pools ({:.1}%)", cache_hits, pools_len, (cache_hits as f64 / pools_len as f64) * 100.0);
            }
            if cache_misses > 0 {
                info!("📊 Redis cache miss: {}/{} pools ({:.1}%)", cache_misses, pools_len, (cache_misses as f64 / pools_len as f64) * 100.0);
            }
            
            if pools_to_fetch.is_empty() {
                return Ok(cached_pools);
            }
            
            // Fetch remaining pools and cache them
            let fetched_pools = self.fetch_pool_states_internal(pools_to_fetch, provider.clone()).await?;
            
            // Cache fetched pools in Redis
            let mut cached_states = Vec::new();
            for pool in &fetched_pools {
                if let Some(cached_state) = self.pool_to_cached_state(pool) {
                    cached_states.push(cached_state);
                }
            }
            if !cached_states.is_empty() {
                // ✅ FIX: No ignorar errores silenciosamente, loggear para diagnóstico
                match redis_guard.batch_cache_pool_states(&cached_states).await {
                    Ok(_) => {
                        info!("✅ Successfully cached {} pool states to Redis", cached_states.len());
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to cache {} pool states to Redis: {}", cached_states.len(), e);
                    }
                }
            }
            
            cached_pools.extend(fetched_pools);
            return Ok(cached_pools);
        }
        
        // Fallback: fetch all pools without Redis
        self.fetch_pool_states_internal(pools, provider).await
    }
    
    #[cfg(feature = "redis")]
    fn pool_to_cached_state(&self, pool: &Pool) -> Option<CachedPoolState> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
        
        match pool {
            Pool::UniswapV2(p) => Some(CachedPoolState {
                address: format!("{:#x}", p.address),
                reserve0: Some(format!("{}", p.reserve0)),
                reserve1: Some(format!("{}", p.reserve1)),
                sqrt_price_x96: None,
                tick: None,
                liquidity: None,
                block_number: 0, // Will be set by caller
                timestamp,
            }),
            Pool::UniswapV3(p) => Some(CachedPoolState {
                address: format!("{:#x}", p.address),
                reserve0: None,
                reserve1: None,
                sqrt_price_x96: Some(format!("{}", p.sqrt_price_x96)),
                tick: Some(p.tick),
                liquidity: Some(format!("{}", p.liquidity)),
                block_number: 0, // Will be set by caller
                timestamp,
            }),
            _ => None,
        }
    }
    
    #[cfg(feature = "redis")]
    fn cached_state_to_pool(&self, cached: &CachedPoolState, original: &Pool) -> Result<Pool> {
        use std::str::FromStr;
        let addr = Address::from_str(&cached.address)?;
        
        match original {
            Pool::UniswapV2(p_orig) => {
                // Convert U256 strings to u128 (UniswapV2Pool uses u128 for reserves)
                let reserve0_u256 = cached.reserve0.as_ref().and_then(|s| U256::from_dec_str(s).ok()).unwrap_or_default();
                let reserve1_u256 = cached.reserve1.as_ref().and_then(|s| U256::from_dec_str(s).ok()).unwrap_or_default();
                let reserve0 = reserve0_u256.try_into().unwrap_or(0u128);
                let reserve1 = reserve1_u256.try_into().unwrap_or(0u128);
                Ok(Pool::UniswapV2(UniswapV2Pool {
                    address: addr,
                    token0: p_orig.token0, // Preserve from original
                    token1: p_orig.token1, // Preserve from original
                    reserve0,
                    reserve1,
                    dex: p_orig.dex.clone(), // Preserve from original
                }))
            }
            Pool::UniswapV3(p_orig) => {
                let sqrt_price_x96 = cached.sqrt_price_x96.as_ref().and_then(|s| U256::from_dec_str(s).ok()).unwrap_or_default();
                let liquidity = cached.liquidity.as_ref().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
                Ok(Pool::UniswapV3(UniswapV3Pool {
                    address: addr,
                    token0: p_orig.token0, // Preserve from original
                    token1: p_orig.token1, // Preserve from original
                    fee: p_orig.fee, // Preserve from original
                    sqrt_price_x96,
                    liquidity,
                    tick: cached.tick.unwrap_or(0),
                    dex: p_orig.dex.clone(), // Preserve from original
                }))
            }
            _ => Err(anyhow::anyhow!("Unsupported pool type for Redis cache")),
        }
    }
    
    async fn fetch_pool_states_internal(&self, pools: Vec<Pool>, provider: Arc<Provider<Http>>) -> Result<Vec<Pool>> {
        // ✅ REFACTOR: Use direct method with optimized batching (no JIT Fetcher)
        // Agrupar pools por tipo para usar multicall eficientemente
        let total_pools = pools.len();
        let mut v2_pools: Vec<(Address, UniswapV2Pool)> = Vec::new();
        let mut v3_pools: Vec<(Address, UniswapV3Pool)> = Vec::new();
        let mut other_pools: Vec<(Address, Pool)> = Vec::new();

        for pool in pools {
            let addr = pool.address();
            match pool {
                Pool::UniswapV2(p) => v2_pools.push((addr, p)),
                Pool::UniswapV3(p) => v3_pools.push((addr, p)),
                p => other_pools.push((addr, p)),
            }
        }

        let mut updated_pools = Vec::with_capacity(total_pools);
        // ✅ Use optimized batch_size (500) and direct fallback for small batches
        const BATCH_SIZE: usize = 500;
        const DIRECT_FALLBACK_V2_LIMIT: usize = 24;
        const DIRECT_FALLBACK_V3_LIMIT: usize = 24;
        
        // ✅ For small batches, use direct RPC calls instead of multicall
        if v2_pools.len() <= DIRECT_FALLBACK_V2_LIMIT && v3_pools.len() <= DIRECT_FALLBACK_V3_LIMIT && other_pools.is_empty() {
            debug!("Using direct RPC fallback for {} V2 and {} V3 pools (small batch)", v2_pools.len(), v3_pools.len());
            return self.fetch_pool_states_direct(v2_pools, v3_pools, other_pools, provider).await;
        }
        
        let multicall = Multicall::new(provider.clone(), self.multicall_address, BATCH_SIZE);

        // Procesar V2 pools con multicall
        if !v2_pools.is_empty() {
            let mut calls = Vec::new();
            let mut pool_map: HashMap<usize, Address> = HashMap::new();
            
            for (idx, (addr, _)) in v2_pools.iter().enumerate() {
                let pair_contract = IUniswapV2Pair::new(*addr, Arc::clone(&provider));
                calls.push(Call {
                    target: *addr,
                    call_data: pair_contract.get_reserves().calldata().unwrap(),
                });
                pool_map.insert(calls.len() - 1, *addr);
            }

            match multicall.run(calls, None).await {
                Ok(results) => {
                    let pair_contract_dummy = IUniswapV2Pair::new(Address::zero(), Arc::clone(&provider));
                    let get_reserves_fn = pair_contract_dummy.abi().function("getReserves")?;
                    for (idx, (addr, mut pool)) in v2_pools.into_iter().enumerate() {
                        if idx < results.len() {
                            if let Ok(decoded) = get_reserves_fn.decode_output(&results[idx]) {
                                if decoded.len() >= 2 {
                                    if let (Some(r0), Some(r1)) = (
                                        decoded[0].clone().into_uint().and_then(|u| u.try_into().ok()),
                                        decoded[1].clone().into_uint().and_then(|u| u.try_into().ok()),
                                    ) {
                                        pool.reserve0 = r0;
                                        pool.reserve1 = r1;
                                    }
                                }
                            }
                        }
                        updated_pools.push(Pool::UniswapV2(pool));
                    }
                }
                Err(e) => {
                    warn!("Multicall failed for V2 pools: {}", e);
                    // Fallback: agregar pools sin actualizar
                    for (_, pool) in v2_pools {
                        updated_pools.push(Pool::UniswapV2(pool));
                    }
                }
            }
        }

        // Procesar V3 pools con multicall
        if !v3_pools.is_empty() {
            let mut calls = Vec::new();
            let mut pool_indices: Vec<(usize, usize)> = Vec::new(); // (slot0_idx, liquidity_idx)
            
            for (addr, _) in &v3_pools {
                let pool_contract = UniswapV3PoolContract::new(*addr, Arc::clone(&provider));
                let slot0_idx = calls.len();
                calls.push(Call {
                    target: *addr,
                    call_data: pool_contract.slot_0().calldata().unwrap(),
                });
                let liquidity_idx = calls.len();
                calls.push(Call {
                    target: *addr,
                    call_data: pool_contract.liquidity().calldata().unwrap(),
                });
                pool_indices.push((slot0_idx, liquidity_idx));
            }

            match multicall.run(calls, None).await {
                Ok(results) => {
                    let pool_contract_dummy = UniswapV3PoolContract::new(Address::zero(), Arc::clone(&provider));
                    let slot0_fn = pool_contract_dummy.abi().function("slot0")?;
                    let liquidity_fn = pool_contract_dummy.abi().function("liquidity")?;
                    
                    for (idx, (addr, mut pool)) in v3_pools.into_iter().enumerate() {
                        if idx < pool_indices.len() {
                            let (slot0_idx, liquidity_idx) = pool_indices[idx];
                            
                            // Decodificar slot0
                            if slot0_idx < results.len() {
                                if let Ok(decoded) = slot0_fn.decode_output(&results[slot0_idx]) {
                                    if decoded.len() >= 2 {
                                        if let Some(sqrt_price) = decoded[0].clone().into_uint() {
                                            pool.sqrt_price_x96 = sqrt_price;
                                        }
                                        if let Some(tick) = decoded[1].clone().into_int().and_then(|i| i.try_into().ok()) {
                                            pool.tick = tick;
                                        }
                                    }
                                }
                            }
                            
                            // Decodificar liquidity
                            if liquidity_idx < results.len() {
                                if let Ok(decoded) = liquidity_fn.decode_output(&results[liquidity_idx]) {
                                    if let Some(liq) = decoded[0].clone().into_uint().and_then(|u| u.try_into().ok()) {
                                        pool.liquidity = liq;
                                    }
                                }
                            }
                        }
                        updated_pools.push(Pool::UniswapV3(pool));
                    }
                }
                Err(e) => {
                    warn!("Multicall failed for V3 pools: {}", e);
                    // Fallback: agregar pools sin actualizar
                    for (_, pool) in v3_pools {
                        updated_pools.push(Pool::UniswapV3(pool));
                    }
                }
            }
        }

        // Procesar otros pools (Balancer, Curve) - mantener comportamiento original por ahora
        for (addr, pool) in other_pools {
            match pool.fetch_state(provider.clone()).await {
                Ok(updated_pool) => updated_pools.push(updated_pool),
                Err(e) => {
                    warn!("Failed to fetch state for pool {:?}: {}", addr, e);
                    // No agregar pools que fallaron - no tienen estado actualizado
                }
            }
        }

        Ok(updated_pools)
    }

    /// ✅ REFACTOR: Direct RPC fallback for small batches (optimization)
    async fn fetch_pool_states_direct(
        &self,
        v2_pools: Vec<(Address, UniswapV2Pool)>,
        v3_pools: Vec<(Address, UniswapV3Pool)>,
        other_pools: Vec<(Address, Pool)>,
        provider: Arc<Provider<Http>>,
    ) -> Result<Vec<Pool>> {
        let mut updated_pools = Vec::new();
        
        // Direct fetch for V2 pools
        for (addr, mut pool) in v2_pools {
            let pair_contract = IUniswapV2Pair::new(addr, Arc::clone(&provider));
            match pair_contract.get_reserves().call().await {
                Ok((r0, r1, _)) => {
                    pool.reserve0 = r0.try_into().unwrap_or(0);
                    pool.reserve1 = r1.try_into().unwrap_or(0);
                }
                Err(e) => {
                    warn!("Direct fetch failed for V2 pool {:?}: {}", addr, e);
                }
            }
            updated_pools.push(Pool::UniswapV2(pool));
        }
        
        // Direct fetch for V3 pools
        for (addr, mut pool) in v3_pools {
            let pool_contract = UniswapV3PoolContract::new(addr, Arc::clone(&provider));
            match pool_contract.slot_0().call().await {
                Ok((sqrt_price, tick, _, _, _, _, _)) => {
                    pool.sqrt_price_x96 = sqrt_price;
                    if let Ok(tick_i32) = tick.try_into() {
                        pool.tick = tick_i32;
                    }
                }
                Err(e) => {
                    warn!("Direct fetch failed for V3 pool {:?} slot0: {}", addr, e);
                }
            }
            match pool_contract.liquidity().call().await {
                Ok(liq) => {
                    if let Ok(liq_u128) = liq.try_into() {
                        pool.liquidity = liq_u128;
                    }
                }
                Err(e) => {
                    warn!("Direct fetch failed for V3 pool {:?} liquidity: {}", addr, e);
                }
            }
            updated_pools.push(Pool::UniswapV3(pool));
        }
        
        // Direct fetch for other pools
        for (_, pool) in other_pools {
            match pool.fetch_state(provider.clone()).await {
                Ok(updated_pool) => updated_pools.push(updated_pool),
                Err(e) => {
                    warn!("Direct fetch failed for pool: {}", e);
                }
            }
        }
        
        Ok(updated_pools)
    }

    async fn get_decimals(&self, tokens: &[Address]) -> Result<HashMap<Address, u8>> {
        if tokens.is_empty() {
            return Ok(HashMap::new());
        }
        
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        // ✅ Usar self.multicall_address desde Settings en lugar de hardcodeado
        let multicall = Multicall::new(provider.clone(), self.multicall_address, 500);

        let erc20 = Erc20::new(Address::zero(), provider);
        
        // ✅ Procesar en chunks si hay muchos tokens (el multicall internamente hace chunks)
        let mut decimals_map = HashMap::new();
        let decimals_fn = erc20.abi().function("decimals")?;
        
        // El multicall.run() internamente hace chunks según batch_size, así que podemos pasar todos los tokens
        let calls: Vec<_> = tokens.iter().map(|&token| {
            multicall::Call {
                target: token,
                call_data: erc20.decimals().calldata().unwrap(),
            }
        }).collect();

        match multicall.run(calls, None).await {
            Ok(results) => {
                for (token, result) in tokens.iter().zip(results) {
                    if let Ok(decoded) = decimals_fn.decode_output(&result) {
                        if let Some(dec) = decoded[0].clone().into_uint().and_then(|u| u.try_into().ok()) {
                            decimals_map.insert(*token, dec);
                        }
                    }
                }
                Ok(decimals_map)
            },
            Err(e) => {
                warn!("Failed to get decimals via multicall for {} tokens: {}. Falling back to empty map.", tokens.len(), e);
                // Retornar mapa vacío en lugar de error - los pools usarán default de 18
                Ok(HashMap::new())
            }
        }
    }

    /// Calculate liquidity USD using pre-loaded prices and decimals (optimized version)
    async fn calculate_liquidity_usd_with_cache(
        &self,
        pool: &Pool,
        prices_map: &HashMap<Address, f64>,
        decimals_map: &HashMap<Address, u8>,
    ) -> Result<f64> {
        match pool {
            Pool::UniswapV2(p) => {
                if p.reserve0 == 0 || p.reserve1 == 0 { return Ok(0.0); }
                let d0 = decimals_map.get(&p.token0).cloned().unwrap_or(18);
                let d1 = decimals_map.get(&p.token1).cloned().unwrap_or(18);
                let price0 = prices_map.get(&p.token0).cloned().unwrap_or(0.0);
                let price1 = prices_map.get(&p.token1).cloned().unwrap_or(0.0);

                let reserve0_f = (p.reserve0 as f64) / 10f64.powi(d0 as i32);
                let reserve1_f = (p.reserve1 as f64) / 10f64.powi(d1 as i32);

                Ok(reserve0_f * price0 + reserve1_f * price1)
            },
            Pool::UniswapV3(p) => {
                if p.liquidity == 0 || p.sqrt_price_x96.is_zero() {
                    return Ok(0.0);
                }
                
                // ✅ CÁLCULO CORRECTO: Calcular balances reales usando fórmulas de UniswapV3
                // Para un pool V3, necesitamos calcular los balances de token0 y token1
                // usando liquidity y sqrt_price_x96
                
                let liquidity_u256 = U256::from(p.liquidity);
                let sqrt_price = p.sqrt_price_x96;
                
                // Calcular balances aproximados asumiendo que el precio está en el tick actual
                // Esta es una aproximación razonable para el cálculo de weight
                // Fórmula simplificada: 
                // - amount0 ≈ liquidity / sqrt_price_x96 (ajustado por Q96)
                // - amount1 ≈ liquidity * sqrt_price_x96 / Q96
                
                // Para obtener una mejor aproximación, usamos el precio actual
                // price = (sqrt_price_x96 / Q96)^2
                // Si price > 1, hay más token1 que token0
                // Si price < 1, hay más token0 que token1
                
                // Calcular precio actual (token1/token0)
                // ✅ FIX: Safe conversion to avoid integer overflow
                let sqrt_price_f64 = if sqrt_price <= U256::from(u128::MAX) {
                    sqrt_price.as_u128() as f64
                } else {
                    u256_to_f64_lossy(sqrt_price)
                };
                let q96_f64 = (1u128 << 96) as f64;
                
                // Calcular balances aproximados
                // Para un pool V3, la relación entre balances es:
                // amount1 / amount0 = price
                // Y la liquidez total en USD es aproximadamente:
                // value ≈ 2 * sqrt(amount0 * amount1 * price0 * price1)
                // O más simple: value ≈ sqrt(amount0 * price0) * sqrt(amount1 * price1) * 2
                
                // Aproximación más directa usando la fórmula de UniswapV3:
                // Para un tick específico, los balances se pueden calcular como:
                // amount0 = L * (sqrt(P_upper) - sqrt(P)) / (sqrt(P_upper) * sqrt(P))
                // amount1 = L * (sqrt(P) - sqrt(P_lower))
                
                // Sin conocer los ticks superior e inferior, usamos una aproximación:
                // Asumimos que el rango es simétrico alrededor del precio actual
                // Esto da una aproximación razonable para el cálculo de weight
                
                let price0 = prices_map.get(&p.token0).cloned().unwrap_or(0.0);
                let price1 = prices_map.get(&p.token1).cloned().unwrap_or(0.0);
                let d0 = decimals_map.get(&p.token0).cloned().unwrap_or(18);
                let d1 = decimals_map.get(&p.token1).cloned().unwrap_or(18);
                
                // Si no tenemos precios, retornar 0
                if price0 == 0.0 && price1 == 0.0 {
                    return Ok(0.0);
                }
                
                // Calcular balances usando la aproximación de UniswapV3
                // Para un pool en el tick actual, podemos aproximar:
                // amount0 ≈ liquidity / sqrt_price (en unidades de token0)
                // amount1 ≈ liquidity * sqrt_price / Q96 (en unidades de token1)
                
                // Conversión más precisa:
                // amount0 = liquidity * Q96 / sqrt_price_x96 (en unidades raw)
                // amount1 = liquidity * sqrt_price_x96 / Q96 (en unidades raw)
                
                let liquidity_f64 = p.liquidity as f64;
                
                // Calcular amount0 y amount1 en unidades raw
                let amount0_raw = if !sqrt_price.is_zero() {
                    let q96_u256 = U256::from(2u128.pow(96));
                    let numerator = liquidity_u256.checked_mul(q96_u256);
                    if let Some(num) = numerator {
                        // Convertir a f64 de forma segura
                        // ✅ FIX: Safe conversion to avoid integer overflow
                        let num_f64 = if num <= U256::from(u128::MAX) {
                            num.as_u128() as f64
                        } else {
                            u256_to_f64_lossy(num)
                        };
                        let sqrt_f64 = if sqrt_price <= U256::from(u128::MAX) {
                            sqrt_price.as_u128() as f64
                        } else {
                            u256_to_f64_lossy(sqrt_price)
                        };
                        if sqrt_f64 > 0.0 {
                            num_f64 / sqrt_f64
                        } else {
                            0.0
                        }
                    } else {
                        // Overflow: usar aproximación
                        (liquidity_f64 * q96_f64) / sqrt_price_f64
                    }
                } else {
                    0.0
                };
                
                let amount1_raw = {
                    let numerator = liquidity_u256.checked_mul(sqrt_price);
                    if let Some(num) = numerator {
                        // ✅ FIX: Safe conversion to avoid integer overflow
                        let num_f64 = if num <= U256::from(u128::MAX) {
                            num.as_u128() as f64
                        } else {
                            u256_to_f64_lossy(num)
                        };
                        num_f64 / q96_f64
                    } else {
                        // Overflow: usar aproximación
                        (liquidity_f64 * sqrt_price_f64) / q96_f64
                    }
                };
                
                // Convertir a unidades con decimales
                let amount0 = amount0_raw / 10f64.powi(d0 as i32);
                let amount1 = amount1_raw / 10f64.powi(d1 as i32);
                
                // Calcular valor USD total
                let value0 = amount0 * price0;
                let value1 = amount1 * price1;
                let total_value = value0 + value1;
                
                Ok(total_value)
            }
            Pool::BalancerWeighted(p) => {
                let mut total_value = 0.0;
                for (idx, token) in p.tokens.iter().enumerate() {
                    if let Some(balance) = p.balances.get(idx) {
                        let price = prices_map.get(token).cloned().unwrap_or(0.0);
                        let decimals = decimals_map.get(token).cloned().unwrap_or(18);
                        // ✅ FIX: Safe conversion to avoid integer overflow
                        let bal_f = if *balance <= U256::from(u128::MAX) {
                            (balance.as_u128() as f64) / 10f64.powi(decimals as i32)
                        } else {
                            u256_to_f64_lossy(*balance) / 10f64.powi(decimals as i32)
                        };
                        total_value += bal_f * price;
                    }
                }
                Ok(total_value)
            }
            Pool::CurveStableSwap(p) => {
                let mut total_value = 0.0;
                for (idx, token) in p.tokens.iter().enumerate() {
                    if let Some(balance) = p.balances.get(idx) {
                        let price = prices_map.get(token).cloned().unwrap_or(0.0);
                        let decimals = decimals_map.get(token).cloned().unwrap_or(18);
                        let bal_f = (balance.as_u128() as f64) / 10f64.powi(decimals as i32);
                        total_value += bal_f * price;
                    }
                }
                Ok(total_value)
            }
        }
    }

    /// Legacy method for backward compatibility (fallback)
    async fn calculate_liquidity_usd(&self, pool: &Pool) -> Result<f64> {
        match pool {
            Pool::UniswapV2(p) => {
                if p.reserve0 == 0 || p.reserve1 == 0 { return Ok(0.0); }
                let tokens = vec![p.token0, p.token1];
                let decimals = self.get_decimals(&tokens).await?;
                let d0 = decimals.get(&p.token0).cloned().unwrap_or(18);
                let d1 = decimals.get(&p.token1).cloned().unwrap_or(18);

                let price0 = self.price_feed.get_usd_price(p.token0).await.unwrap_or(0.0);
                let price1 = self.price_feed.get_usd_price(p.token1).await.unwrap_or(0.0);

                let reserve0_f = (p.reserve0 as f64) / 10f64.powi(d0 as i32);
                let reserve1_f = (p.reserve1 as f64) / 10f64.powi(d1 as i32);

                Ok(reserve0_f * price0 + reserve1_f * price1)
            },
            Pool::UniswapV3(p) => {
                if p.liquidity == 0 { return Ok(0.0); }
                let price0 = self.price_feed.get_usd_price(p.token0).await.unwrap_or(0.0);
                let d0 = self.get_decimals(&[p.token0]).await?.get(&p.token0).cloned().unwrap_or(18);
                let liquidity_f = (p.liquidity as f64) / 10f64.powi(d0 as i32);
                Ok(liquidity_f * price0)
            }
            Pool::BalancerWeighted(p) => {
                let mut total_value = 0.0;
                for (idx, token) in p.tokens.iter().enumerate() {
                    if let Some(balance) = p.balances.get(idx) {
                        let price = self.price_feed.get_usd_price(*token).await.unwrap_or(0.0);
                        let decimals = self.get_decimals(&[*token]).await?.get(token).cloned().unwrap_or(18);
                        let bal_f = (balance.as_u128() as f64) / 10f64.powi(decimals as i32);
                        total_value += bal_f * price;
                    }
                }
                Ok(total_value)
            }
            Pool::CurveStableSwap(p) => {
                let mut total_value = 0.0;
                for (idx, token) in p.tokens.iter().enumerate() {
                    if let Some(balance) = p.balances.get(idx) {
                        let price = self.price_feed.get_usd_price(*token).await.unwrap_or(0.0);
                        let decimals = self.get_decimals(&[*token]).await?.get(token).cloned().unwrap_or(18);
                        let bal_f = (balance.as_u128() as f64) / 10f64.powi(decimals as i32);
                        total_value += bal_f * price;
                    }
                }
                Ok(total_value)
            }
        }
    }
}