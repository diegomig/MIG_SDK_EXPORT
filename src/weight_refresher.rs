//! Weight Refresher: Historical weight update tasks
//!
//! Provides functions to refresh weights for hot/warm pools using external price APIs
//! (CoinGecko) with fallback to Chainlink, and RPC for pool state.

use crate::database::{self, DbPool, PoolCandidate};
use crate::graph_service::GraphService;
use crate::hot_pool_manager::HotPoolManager;
use crate::price_feeds::PriceFeed;
use crate::rpc_pool::RpcPool;
use crate::flight_recorder::FlightRecorder;
use crate::{record_phase_start, record_phase_end};
use anyhow::Result;
use ethers::prelude::{Address, Provider, Http, Middleware};
use std::sync::Arc;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn, debug};

/// Refresh weights for hot pools (top N by weight)
///
/// # Parameters
/// - `top_n`: Number of top pools to refresh (default: 50)
/// - `min_weight`: Minimum weight threshold (default: $100K)
pub async fn refresh_hot_pools<M>(
    graph_service: &GraphService<M>,
    db_pool: &DbPool,
    rpc_pool: Arc<RpcPool>,
    top_n: usize,
    min_weight: f64,
    flight_recorder: Option<Arc<FlightRecorder>>,
) -> Result<usize>
where
    M: ethers::prelude::Middleware + 'static,
{
    let start = Instant::now();
    info!("🔥 Starting hot pools refresh (top {}, min weight: ${:.0})", top_n, min_weight);

    // 1. Load top N pools by historical weight
    let candidates = database::load_pool_candidates(db_pool, min_weight, top_n as i64).await?;
    
    if candidates.is_empty() {
        warn!("⚠️ No hot pool candidates found");
        return Ok(0);
    }

    info!("📊 Loaded {} hot pool candidates", candidates.len());

    // 2. Convert to addresses
    let addresses: Vec<Address> = candidates.iter().map(|c| c.address).collect();

    // 3. Load complete pools from database
    let pools = database::load_pools_by_addresses(db_pool, &addresses).await?;
    
    if pools.is_empty() {
        warn!("⚠️ No pools found in database for {} candidates", candidates.len());
        return Ok(0);
    }

    // 4. Fetch pool states on-chain (RPC required)
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    // Note: fetch_pool_states expects Arc<Provider<Http>>, provider is already Arc
    // But GraphService<M> where M is Provider<Http> (not Arc), so we need to clone
    let pools_with_state = graph_service.fetch_pool_states(pools, provider.clone()).await?;

    let failed_validation = addresses.len() - pools_with_state.len();
    if failed_validation > 0 {
        let failure_rate = (failed_validation as f64 / addresses.len() as f64) * 100.0;
        warn!("⚠️ {} pools failed on-chain validation ({:.1}%)", failed_validation, failure_rate);
    }

    // 5. Calculate and update weights
    // Use incremental update which handles price fetching internally
    let pool_addresses: Vec<Address> = pools_with_state.iter().map(|p| p.address()).collect();
    
    // ✅ FLIGHT RECORDER: Capturar recorder para usar en match
    let recorder_ref = flight_recorder.as_ref();
    
    match graph_service.calculate_and_update_weights_for_pools(&pool_addresses).await {
        Ok(_) => {
            let duration = start.elapsed();
            info!("✅ Hot pools refresh completed: {} pools updated in {:?}", pool_addresses.len(), duration);
            
            // ✅ FLIGHT RECORDER: Registrar fin de hot pools refresh
            if let Some(recorder) = recorder_ref {
                record_phase_end!(recorder, "weight_refresh_hot", start, serde_json::json!({
                    "pools_updated": pool_addresses.len(),
                    "candidates_loaded": candidates.len(),
                    "failed_validation": failed_validation,
                    "duration_ms": duration.as_millis()
                }));
            }
            
            Ok(pool_addresses.len())
        }
        Err(e) => {
            warn!("❌ Hot pools refresh failed: {}", e);
            
            // ✅ FLIGHT RECORDER: Registrar error
            if let Some(recorder) = recorder_ref {
                record_phase_end!(recorder, "weight_refresh_hot", start, serde_json::json!({
                    "error": e.to_string(),
                    "duration_ms": start.elapsed().as_millis()
                }));
            }
            
            Err(e)
        }
    }
}

/// Refresh weights for warm pools (mid-tier by weight)
///
/// # Parameters
/// - `min_weight`: Minimum weight threshold (default: $10K)
/// - `max_weight`: Maximum weight threshold (default: $100K)
/// - `limit`: Maximum number of pools to refresh (default: 150)
pub async fn refresh_warm_pools<M>(
    graph_service: &GraphService<M>,
    db_pool: &DbPool,
    rpc_pool: Arc<RpcPool>,
    min_weight: f64,
    max_weight: f64,
    limit: usize,
    flight_recorder: Option<Arc<FlightRecorder>>,
) -> Result<usize>
where
    M: ethers::prelude::Middleware + 'static,
{
    let start = Instant::now();
    info!("🌡️ Starting warm pools refresh (weight: ${:.0}-${:.0}, limit: {})", min_weight, max_weight, limit);
    
    // ✅ FLIGHT RECORDER: Registrar inicio de warm pools refresh
    if let Some(ref recorder) = flight_recorder {
        record_phase_start!(recorder, "weight_refresh_warm", serde_json::json!({
            "min_weight": min_weight,
            "max_weight": max_weight,
            "limit": limit
        }));
    }

    // Load pools in weight range
    // Note: We need to add a function to load pools by weight range
    // For now, load candidates and filter by weight range
    let candidates = database::load_pool_candidates(db_pool, min_weight, (limit * 2) as i64).await?;
    
    // Filter to weight range
    let filtered_candidates: Vec<PoolCandidate> = candidates
        .into_iter()
        .filter(|c| c.weight >= min_weight && c.weight < max_weight)
        .take(limit)
        .collect();

    if filtered_candidates.is_empty() {
        warn!("⚠️ No warm pool candidates found in range");
        return Ok(0);
    }

    info!("📊 Loaded {} warm pool candidates", filtered_candidates.len());

    // Convert to addresses
    let addresses: Vec<Address> = filtered_candidates.iter().map(|c| c.address).collect();

    // Load complete pools
    let pools = database::load_pools_by_addresses(db_pool, &addresses).await?;
    
    if pools.is_empty() {
        warn!("⚠️ No pools found in database for {} candidates", filtered_candidates.len());
        return Ok(0);
    }

    // Fetch pool states
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    let pools_with_state = graph_service.fetch_pool_states(pools, provider).await?;

    // Calculate and update weights
    let pool_addresses: Vec<Address> = pools_with_state.iter().map(|p| p.address()).collect();
    
    // ✅ FLIGHT RECORDER: Capturar recorder para usar en match
    let recorder_ref = flight_recorder.as_ref();
    
    match graph_service.calculate_and_update_weights_for_pools(&pool_addresses).await {
        Ok(_) => {
            let duration = start.elapsed();
            info!("✅ Warm pools refresh completed: {} pools updated in {:?}", pool_addresses.len(), duration);
            
            // ✅ FLIGHT RECORDER: Registrar fin de warm pools refresh
            if let Some(recorder) = recorder_ref {
                record_phase_end!(recorder, "weight_refresh_warm", start, serde_json::json!({
                    "pools_updated": pool_addresses.len(),
                    "candidates_loaded": filtered_candidates.len(),
                    "duration_ms": duration.as_millis()
                }));
            }
            
            Ok(pool_addresses.len())
        }
        Err(e) => {
            warn!("❌ Warm pools refresh failed: {}", e);
            
            // ✅ FLIGHT RECORDER: Registrar error
            if let Some(recorder) = recorder_ref {
                record_phase_end!(recorder, "weight_refresh_warm", start, serde_json::json!({
                    "error": e.to_string(),
                    "duration_ms": start.elapsed().as_millis()
                }));
            }
            
            Err(e)
        }
    }
}

/// Refresh weights and repopulate Hot Pool Manager
pub async fn refresh_and_repopulate_hot_pool_manager(
    _hot_pool_manager: &HotPoolManager,
    graph_service: &GraphService<Arc<Provider<Http>>>,
    db_pool: &DbPool,
    rpc_pool: Arc<RpcPool>,
    flight_recorder: Option<Arc<FlightRecorder>>,
) -> Result<usize> {
    // First refresh hot pools
    refresh_hot_pools(graph_service, db_pool, rpc_pool.clone(), 50, 100_000.0, flight_recorder).await?;
    
    // Then repopulate Hot Pool Manager
    // Use the existing populate function from background_discoverer
    // For now, we'll call it directly - in production, extract to shared module
    Ok(0) // Placeholder - will be implemented with shared populate function
}
