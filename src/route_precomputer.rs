//! # Route Precomputer
//!
//! Pre-computes triangular routes (A→B→C→A) from active pools and caches them in Redis.
//! This eliminates on-demand route calculation which is 10-100x slower.
//!
//! ## Features
//!
//! - Pre-computes all triangular routes from active pools
//! - Scores routes based on liquidity (geometric mean for V2, direct liquidity for V3)
//! - Caches top-N routes in Redis with TTL
//! - Optimized batch retrieval using MGET
//! - Integrates with PoolValidationCache for pool validation

use anyhow::{Context, Result};
use ethers::types::Address;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::database::DbPool;
use crate::pool_validation_cache::PoolValidationCache;
use crate::pools::Pool;
use crate::router::{CandidateRoute, DexId, SwapKind, SwapStep};
use std::sync::Arc;

#[cfg(feature = "redis")]
use redis::cmd as redis_cmd;
#[cfg(feature = "redis")]
use redis::{aio::ConnectionManager, Client, Value};

/// Pre-computed triangular route with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecomputedTriangularRoute {
    pub route_id: String,
    pub steps: Vec<SerializableSwapStep>,
    pub entry_token: Address,
    pub pool_a: Address,
    pub pool_b: Address,
    pub pool_c: Address,
    pub token_in: Address,
    pub token_mid: Address,
    pub token_out: Address,
    pub computed_at_block: u64,
    pub route_score: f64, // Composite score for ranking
}

/// Serializable version of SwapStep for Redis caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableSwapStep {
    #[serde(with = "address_serde")]
    pub pool: Address,
    #[serde(with = "address_serde")]
    pub token_in: Address,
    #[serde(with = "address_serde")]
    pub token_out: Address,
    pub dex: String,
    pub fee_bps: u32,
    pub kind: String,
}

mod address_serde {
    use ethers::types::Address;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(address: &Address, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{:#x}", address))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Address, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl SerializableSwapStep {
    pub fn from_swap_step(step: &SwapStep) -> Self {
        Self {
            pool: step.pool,
            token_in: step.token_in,
            token_out: step.token_out,
            dex: format!("{:?}", step.dex),
            fee_bps: step.fee_bps,
            kind: format!("{:?}", step.kind),
        }
    }

    pub fn to_swap_step(&self) -> SwapStep {
        // Parse DEX from string
        let dex = match self.dex.as_str() {
            "UniswapV2" => DexId::UniswapV2,
            "UniswapV3" => DexId::UniswapV3,
            "SushiSwapV2" => DexId::SushiSwapV2,
            "CamelotV2" => DexId::CamelotV2,
            "CamelotV3" => DexId::CamelotV3,
            "PancakeSwap" => DexId::PancakeSwap,
            "KyberSwap" => DexId::KyberSwap,
            "Curve" => DexId::Curve,
            "Balancer" => DexId::Balancer,
            _ => DexId::UniswapV2, // Default fallback
        };

        // Parse SwapKind from string
        let kind = match self.kind.as_str() {
            "V2" => SwapKind::V2,
            "V3" => SwapKind::V3,
            "Balancer" => SwapKind::Balancer,
            "Curve" => SwapKind::Curve,
            _ => SwapKind::V2, // Default fallback
        };

        SwapStep {
            dex,
            pool: self.pool,
            token_in: self.token_in,
            token_out: self.token_out,
            fee_bps: self.fee_bps,
            kind,
            reserve_in: ethers::types::U256::zero(),
            reserve_out: ethers::types::U256::zero(),
            weight: 1.0,
            pool_id: None,
            token_indices: None,
        }
    }
}

impl PrecomputedTriangularRoute {
    pub fn to_candidate_route(&self) -> CandidateRoute {
        CandidateRoute {
            steps: self.steps.iter().map(|s| s.to_swap_step()).collect(),
            entry_token: self.entry_token,
        }
    }
}

/// Route Precomputer - Pre-computes all triangular routes and caches them
#[cfg(feature = "redis")]
pub struct RoutePrecomputer {
    redis: ConnectionManager,
    db_pool: DbPool,
    cache_ttl_seconds: u64,
    pool_validation_cache: Arc<PoolValidationCache>,
}

#[cfg(feature = "redis")]
impl RoutePrecomputer {
    pub async fn new(
        redis_url: &str,
        db_pool: DbPool,
        cache_ttl_seconds: u64,
        pool_validation_cache: Arc<PoolValidationCache>,
    ) -> Result<Self> {
        let client = Client::open(redis_url).context("Failed to create Redis client")?;
        let redis = ConnectionManager::new(client)
            .await
            .context("Failed to connect to Redis")?;

        info!("✅ RoutePrecomputer connected to Redis at {}", redis_url);

        Ok(Self {
            redis,
            db_pool,
            cache_ttl_seconds,
            pool_validation_cache,
        })
    }

    /// Pre-computes ALL triangular routes from given pools
    /// Returns number of routes found
    /// Processes all pools without limit, selects top N routes by score
    pub async fn precompute_all_triangular_routes(
        &mut self,
        pools: &[Pool],
        current_block: u64,
        top_n: Option<usize>,
    ) -> Result<usize> {
        let start = Instant::now();
        info!(
            "🔍 Starting triangular route pre-computation with {} pools...",
            pools.len()
        );

        // Build token → pools index for fast lookup
        // FILTRO: Validar pools usando cache antes de incluirlos
        let mut token_to_pools: HashMap<Address, Vec<(Address, Pool)>> = HashMap::new();
        let mut processed_pools = 0;
        let mut invalid_pools_skipped = 0;

        for pool in pools {
            // Validar pool usando cache
            let pool_addr = pool.address();
            if let Some(is_valid) = self.pool_validation_cache.get(&pool_addr, current_block) {
                if !is_valid {
                    invalid_pools_skipped += 1;
                    continue; // Skip invalid pools
                }
            } else {
                // Cache miss: asumir válido por ahora (background validator lo validará)
            }
            let (token0, token1) = match pool {
                Pool::UniswapV2(p) => (p.token0, p.token1),
                Pool::UniswapV3(p) => (p.token0, p.token1),
                Pool::CurveStableSwap(_) => {
                    // Curve pools have multiple tokens, skip for now (can extend later)
                    continue;
                }
                Pool::BalancerWeighted(_) => {
                    // Balancer pools have multiple tokens, skip for now (can extend later)
                    continue;
                }
            };

            token_to_pools
                .entry(token0)
                .or_default()
                .push((token1, pool.clone()));
            token_to_pools
                .entry(token1)
                .or_default()
                .push((token0, pool.clone()));
            processed_pools += 1;

            // Log progress every 1000 pools
            if processed_pools % 1000 == 0 {
                info!(
                    "📊 Processed {} pools, built token index...",
                    processed_pools
                );
            }
        }

        info!("📊 Built token index: {} unique tokens from {} processed pools ({} invalid pools skipped)",
              token_to_pools.len(), processed_pools, invalid_pools_skipped);

        if token_to_pools.len() < 3 {
            warn!(
                "⚠️ Not enough tokens ({}) to form triangular routes. Need at least 3.",
                token_to_pools.len()
            );
            return Ok(0);
        }

        if processed_pools < 3 {
            warn!(
                "⚠️ Not enough pools ({}) to form triangular routes. Need at least 3.",
                processed_pools
            );
            return Ok(0);
        }

        let mut routes = Vec::new();
        let mut route_ids = HashSet::new();

        // Find all triangular routes: A → B → C → A
        // For each token_in, find all pools that contain it
        let mut routes_found = 0;
        for (token_in, pools_with_token_in) in &token_to_pools {
            for (token_mid, pool_a) in pools_with_token_in {
                // pool_a connects token_in → token_mid

                // OPTIMIZATION: Early skip if token_mid has no connections
                let pools_with_token_mid = match token_to_pools.get(token_mid) {
                    Some(pools) => pools,
                    None => continue,
                };

                for (token_out, pool_b) in pools_with_token_mid {
                    if pool_b.address() == pool_a.address() {
                        continue; // Skip same pool
                    }

                    // pool_b connects token_mid → token_out

                    // OPTIMIZATION: Early skip if token_out has no connection back to token_in
                    let pools_with_token_out = match token_to_pools.get(token_out) {
                        Some(pools) => pools,
                        None => continue,
                    };

                    // OPTIMIZATION: Check if any pool closes the triangle before iterating
                    let closes_triangle =
                        pools_with_token_out.iter().any(|(other_token, pool_c)| {
                            *other_token == *token_in
                                && pool_c.address() != pool_a.address()
                                && pool_c.address() != pool_b.address()
                        });

                    if !closes_triangle {
                        continue; // Skip if no pool closes the triangle
                    }

                    // Now iterate to find the actual closing pool
                    for (other_token, pool_c) in pools_with_token_out {
                        if pool_c.address() == pool_a.address()
                            || pool_c.address() == pool_b.address()
                        {
                            continue; // Skip already used pools
                        }

                        // Verify pool_c closes the triangle: token_out → token_in
                        if *other_token == *token_in {
                            // Create route ID (normalize to avoid duplicates)
                            let mut pool_addrs =
                                vec![pool_a.address(), pool_b.address(), pool_c.address()];
                            pool_addrs.sort();
                            let route_id =
                                format!("{}-{}-{}", pool_addrs[0], pool_addrs[1], pool_addrs[2]);

                            // Skip duplicates
                            if route_ids.contains(&route_id) {
                                continue;
                            }
                            route_ids.insert(route_id.clone());

                            // Create SwapSteps
                            let step_a = self.create_swap_step(pool_a, *token_in, *token_mid)?;
                            let step_b = self.create_swap_step(pool_b, *token_mid, *token_out)?;
                            let step_c = self.create_swap_step(pool_c, *token_out, *token_in)?;

                            // Calculate route score (enhanced: based on liquidity)
                            let score = self.calculate_route_score(pool_a, pool_b, pool_c);

                            let route = PrecomputedTriangularRoute {
                                route_id: route_id.clone(),
                                steps: vec![
                                    SerializableSwapStep::from_swap_step(&step_a),
                                    SerializableSwapStep::from_swap_step(&step_b),
                                    SerializableSwapStep::from_swap_step(&step_c),
                                ],
                                entry_token: *token_in,
                                pool_a: pool_a.address(),
                                pool_b: pool_b.address(),
                                pool_c: pool_c.address(),
                                token_in: *token_in,
                                token_mid: *token_mid,
                                token_out: *token_out,
                                computed_at_block: current_block,
                                route_score: score,
                            };

                            routes.push(route);
                            routes_found += 1;

                            // Log progress every 1000 routes
                            if routes_found % 1000 == 0 {
                                info!("📊 Found {} routes so far...", routes_found);
                            }
                        }
                    }
                }
            }
        }

        if routes.is_empty() {
            warn!("⚠️ No triangular routes found with {} pools and {} tokens. This may indicate insufficient connectivity between pools.",
                  processed_pools, token_to_pools.len());
        } else {
            info!(
                "✅ Pre-computed {} triangular routes in {:?}",
                routes.len(),
                start.elapsed()
            );
        }

        // Select top N routes by score
        let routes_to_cache = if let Some(n) = top_n {
            if routes.len() > n {
                info!(
                    "📊 Selecting top {} routes from {} total routes",
                    n,
                    routes.len()
                );
                // Sort by score descending and take top N
                routes.sort_by(|a, b| {
                    b.route_score
                        .partial_cmp(&a.route_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                routes.truncate(n);
                routes
            } else {
                routes
            }
        } else {
            routes
        };

        // Cache top N routes to Redis
        let cached_count = if !routes_to_cache.is_empty() {
            self.cache_routes_to_redis(
                &routes_to_cache,
                current_block,
                processed_pools,
                routes_found,
                routes_to_cache.len(),
            )
            .await?;
            routes_to_cache.len()
        } else {
            0
        };

        Ok(cached_count)
    }

    /// Create SwapStep from pool and tokens
    fn create_swap_step(
        &self,
        pool: &Pool,
        token_in: Address,
        token_out: Address,
    ) -> Result<SwapStep> {
        match pool {
            Pool::UniswapV2(p) => {
                // V2 pools don't have fee field, use default 30 bps (0.3%)
                let fee_bps = 30;
                Ok(SwapStep {
                    dex: DexId::UniswapV2,
                    pool: p.address,
                    token_in,
                    token_out,
                    fee_bps,
                    kind: SwapKind::V2,
                    reserve_in: ethers::types::U256::zero(),
                    reserve_out: ethers::types::U256::zero(),
                    weight: 1.0,
                    pool_id: None,
                    token_indices: None,
                })
            }
            Pool::UniswapV3(p) => Ok(SwapStep {
                dex: DexId::UniswapV3,
                pool: p.address,
                token_in,
                token_out,
                fee_bps: p.fee,
                kind: SwapKind::V3,
                reserve_in: ethers::types::U256::zero(),
                reserve_out: ethers::types::U256::zero(),
                weight: 1.0,
                pool_id: None,
                token_indices: None,
            }),
            _ => anyhow::bail!("Unsupported pool type for route pre-computation"),
        }
    }

    /// Calculate route score based on liquidity/volume (enhanced heuristic)
    fn calculate_route_score(&self, pool_a: &Pool, pool_b: &Pool, pool_c: &Pool) -> f64 {
        // Enhanced scoring: combine liquidity estimates from all three pools
        let mut total_score = 0.0;
        let mut pool_count = 0;

        for pool in [pool_a, pool_b, pool_c].iter() {
            let pool_score = match pool {
                Pool::UniswapV2(p) => {
                    // V2: Use geometric mean of reserves as liquidity proxy
                    let reserve0 = p.reserve0 as f64;
                    let reserve1 = p.reserve1 as f64;
                    if reserve0 > 0.0 && reserve1 > 0.0 {
                        (reserve0 * reserve1).sqrt()
                    } else {
                        0.0
                    }
                }
                Pool::UniswapV3(p) => {
                    // V3: Use liquidity directly (already in correct units)
                    p.liquidity as f64
                }
                _ => {
                    0.0 // Curve/Balancer: skip for now
                }
            };

            if pool_score > 0.0 {
                total_score += pool_score;
                pool_count += 1;
            }
        }

        // Average score across pools, with bonus for all pools having liquidity
        if pool_count > 0 {
            let avg_score = total_score / pool_count as f64;
            // Bonus multiplier if all 3 pools have liquidity
            let bonus = if pool_count == 3 { 1.2 } else { 1.0 };
            // Normalize using logarithmic scale to handle very large values
            // Use log10 to compress large values into 0-100 range
            let max_expected_score: f64 = 1e15; // Maximum expected geo_mean or liquidity
            let normalized = if avg_score > 0.0 {
                let scaled: f64 = avg_score * bonus;
                // Use log10(1 + x) to avoid log(0) and handle small values
                let log_score: f64 = (1.0 + scaled).log10();
                let log_max: f64 = (1.0 + max_expected_score).log10();
                // Normalize to 0-100 range
                (100.0 * log_score / log_max).min(100.0).max(0.0)
            } else {
                0.0
            };
            normalized
        } else {
            0.0
        }
    }

    /// Cache routes to Redis
    async fn cache_routes_to_redis(
        &mut self,
        routes: &[PrecomputedTriangularRoute],
        current_block: u64,
        total_pools_processed: usize,
        total_routes_computed: usize,
        top_n_routes_selected: usize,
    ) -> Result<()> {
        let start = Instant::now();

        // Use simpler key for top routes
        let key = "routes:top:latest";

        // Clear old routes
        let _: () = redis_cmd("DEL")
            .arg(key)
            .query_async(&mut self.redis)
            .await
            .context("Failed to clear old routes")?;

        if routes.is_empty() {
            warn!("⚠️ No routes to cache");
            return Ok(());
        }

        // Serialize all routes first (more efficient than per-chunk)
        let serialized_routes: Vec<(String, String)> = routes
            .iter()
            .map(|route| {
                let route_json =
                    serde_json::to_string(route).context("Failed to serialize route")?;
                let route_key = format!("route:triangular:{}", route.route_id);
                Ok((route_key, route_json))
            })
            .collect::<Result<Vec<_>>>()?;

        // BATCHING: Process in chunks to avoid Redis pipeline size limits
        const BATCH_SIZE: usize = 100;

        let total_batches = (routes.len() + BATCH_SIZE - 1) / BATCH_SIZE;
        debug!(
            "🔄 Caching {} routes in {} batches of ~{}",
            routes.len(),
            total_batches,
            BATCH_SIZE
        );

        for (batch_idx, chunk) in routes.chunks(BATCH_SIZE).enumerate() {
            // Get corresponding serialized routes for this chunk
            let start_idx = batch_idx * BATCH_SIZE;
            let end_idx = (start_idx + chunk.len()).min(serialized_routes.len());
            let chunk_serialized = &serialized_routes[start_idx..end_idx];

            // First, execute SET operations
            let mut set_pipe = redis::pipe();
            set_pipe.atomic();
            for (route_key, route_json) in chunk_serialized {
                set_pipe.set_ex(route_key, route_json, self.cache_ttl_seconds);
            }

            match set_pipe.query_async::<_, Vec<Value>>(&mut self.redis).await {
                Ok(_) => {
                    debug!(
                        "✅ SET batch {}/{} completed ({} routes)",
                        batch_idx + 1,
                        total_batches,
                        chunk.len()
                    );
                }
                Err(e) => {
                    error!(
                        "❌ SET batch {}/{} failed: {:?}",
                        batch_idx + 1,
                        total_batches,
                        e
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to execute SET batch {}/{}: {}",
                        batch_idx + 1,
                        total_batches,
                        e
                    ));
                }
            }

            // Execute ZADD operations
            let mut zadd_success = 0;
            let mut zadd_failed = 0;

            for route in chunk.iter() {
                // Sanitize score: validate if normal. If NaN or Inf, force to 0.0
                let safe_score = if route.route_score.is_finite() {
                    route.route_score
                } else {
                    warn!(
                        "⚠️ Score is NaN or Inf for route {}, forcing to 0.0",
                        route.route_id
                    );
                    0.0
                };

                // Convert to integer: multiply by 1,000,000 to maintain 6 decimal precision
                let score_int = (safe_score * 1_000_000.0) as i64;

                // Execute with explicit protocol (Low Level)
                match redis_cmd("ZADD")
                    .arg(key) // Arg 1: Key
                    .arg(score_int) // Arg 2: Score (Integer, infallible)
                    .arg(&route.route_id) // Arg 3: Member
                    .query_async::<_, ()>(&mut self.redis)
                    .await
                {
                    Ok(_) => {
                        zadd_success += 1;
                    }
                    Err(e) => {
                        zadd_failed += 1;
                        error!("❌ ZADD failed. Key: {}, Score (int): {}, Score (float): {}, ID: {}. Error: {:?}",
                              key, score_int, route.route_score, route.route_id, e);
                        // Continue with other routes instead of failing entire batch
                    }
                }
            }

            if zadd_failed > 0 {
                warn!(
                    "⚠️ ZADD batch {}/{}: {} succeeded, {} failed",
                    batch_idx + 1,
                    total_batches,
                    zadd_success,
                    zadd_failed
                );
            } else {
                debug!(
                    "✅ ZADD batch {}/{} completed ({} routes)",
                    batch_idx + 1,
                    total_batches,
                    chunk.len()
                );
            }

            debug!(
                "✅ Batch {}/{} completed successfully (SET + ZADD, {} routes)",
                batch_idx + 1,
                total_batches,
                chunk.len()
            );
        }

        // Keep sorted-set lifetime aligned with route payload lifetime
        redis_cmd("EXPIRE")
            .arg(key)
            .arg(self.cache_ttl_seconds as i64)
            .query_async::<_, ()>(&mut self.redis)
            .await
            .context("Failed to EXPIRE routes:top:latest")?;

        // Enhanced metadata (separate operation, not critical path)
        let metadata = serde_json::json!({
            "total_pools_processed": total_pools_processed,
            "total_routes_computed": total_routes_computed,
            "top_n_routes_selected": top_n_routes_selected,
            "computed_at_block": current_block,
            "computed_at": chrono::Utc::now().to_rfc3339(),
            "updated_at": chrono::Utc::now().timestamp(),
        });

        redis_cmd("SET")
            .arg("routes:top:metadata")
            .arg(serde_json::to_string(&metadata)?)
            .arg("EX")
            .arg(self.cache_ttl_seconds)
            .query_async::<_, ()>(&mut self.redis)
            .await
            .context("Failed to cache metadata")?;

        let duration = start.elapsed();
        info!(
            "✅ Cached {} top routes to Redis (from {} computed, {} pools processed) in {:?}",
            routes.len(),
            total_routes_computed,
            total_pools_processed,
            duration
        );
        Ok(())
    }

    /// Get top routes from Redis
    /// OPTIMIZED: Uses MGET for batch retrieval and parallel deserialization
    pub async fn get_top_routes_from_redis(
        &mut self,
        limit: Option<usize>,
    ) -> Result<Vec<CandidateRoute>> {
        let start = Instant::now();
        let key = "routes:top:latest";

        // Get route IDs from sorted set (sorted by score, descending)
        let route_ids: Vec<String> = if let Some(limit) = limit {
            redis_cmd("ZREVRANGE")
                .arg(key)
                .arg(0)
                .arg(if limit > 0 { (limit - 1) as i64 } else { 0 })
                .query_async(&mut self.redis)
                .await
                .context("Failed to get route IDs from sorted set")?
        } else {
            redis_cmd("ZREVRANGE")
                .arg(key)
                .arg(0)
                .arg(-1)
                .query_async(&mut self.redis)
                .await
                .context("Failed to get route IDs from sorted set")?
        };

        if route_ids.is_empty() {
            info!("📥 Retrieved 0 routes from Redis (empty sorted set)");
            return Ok(Vec::new());
        }

        // Build all keys for MGET
        let route_keys: Vec<String> = route_ids
            .iter()
            .map(|route_id| format!("route:triangular:{}", route_id))
            .collect();

        // MGET all routes in a single network call
        let json_values: Vec<Option<String>> = redis_cmd("MGET")
            .arg(&route_keys)
            .query_async(&mut self.redis)
            .await
            .context("Failed to MGET routes from Redis")?;

        let mget_duration = start.elapsed();

        // Identify dangling IDs (sorted-set member exists but payload missing in Redis)
        let mut dangling_ids: Vec<String> = Vec::new();
        for (idx, json_opt) in json_values.iter().enumerate() {
            if json_opt.is_none() {
                if let Some(route_id) = route_ids.get(idx) {
                    dangling_ids.push(route_id.clone());
                }
            }
        }

        // Parallel deserialization using rayon
        use rayon::prelude::*;
        let deserialize_start = Instant::now();
        let routes: Vec<CandidateRoute> = json_values
            .into_par_iter()
            .filter_map(|json_opt| {
                json_opt.and_then(|json| {
                    serde_json::from_str::<PrecomputedTriangularRoute>(&json)
                        .ok()
                        .map(|route| route.to_candidate_route())
                })
            })
            .collect();
        let deserialize_duration = deserialize_start.elapsed();
        let total_duration = start.elapsed();

        info!("📥 Retrieved {} top routes from Redis in {:?} (MGET: {:?}, deserialize: {:?}, parallel)",
              routes.len(), total_duration, mget_duration, deserialize_duration);

        if !dangling_ids.is_empty() {
            // ZREM supports variadic members
            let mut cmd = redis_cmd("ZREM");
            cmd.arg(key);
            for id in &dangling_ids {
                cmd.arg(id);
            }
            // Best-effort: pruning failures should not break route loading
            if let Err(e) = cmd.query_async::<_, ()>(&mut self.redis).await {
                warn!("⚠️ Failed to prune dangling route IDs: {}", e);
            }
            warn!(
                "🧹 Pruned {} dangling route IDs from {} (payload missing)",
                dangling_ids.len(),
                key
            );
        }
        Ok(routes)
    }

    /// Invalidate cache (when pools change)
    pub async fn invalidate_cache(&mut self) -> Result<()> {
        // Get all route keys
        let keys: Vec<String> = redis_cmd("KEYS")
            .arg("route:triangular:*")
            .query_async(&mut self.redis)
            .await
            .context("Failed to get route keys")?;

        if !keys.is_empty() {
            redis_cmd("DEL")
                .arg(&keys)
                .query_async::<_, ()>(&mut self.redis)
                .await
                .context("Failed to delete route keys")?;
        }

        // Delete sorted set
        let _: () = redis_cmd("DEL")
            .arg("routes:top:latest")
            .query_async(&mut self.redis)
            .await
            .context("Failed to delete sorted set")?;

        // Delete metadata
        let _: () = redis_cmd("DEL")
            .arg("routes:top:metadata")
            .query_async(&mut self.redis)
            .await
            .context("Failed to delete metadata")?;

        info!("🗑️ Invalidated route cache");
        Ok(())
    }
}
