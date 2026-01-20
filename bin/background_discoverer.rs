//! # Background Discoverer Service
//!
//! Continuous service that runs discovery cycles and graph weight updates
//! in the background for the MIG Topology SDK.
//!
//! ## Overview
//!
//! This service:
//! - Runs discovery cycles periodically (configurable via `discovery.interval_seconds`)
//! - Updates graph weights periodically (configurable via `graph.update_interval_seconds`)
//! - Handles graceful shutdown on Ctrl+C
//!
//! ## Usage
//!
//! ```bash
//! cargo run --bin background_discoverer
//! ```
//!
//! Press Ctrl+C to stop gracefully.

use mig_topology_sdk::{
    adapters::{uniswap_v2::UniswapV2Adapter, uniswap_v3::UniswapV3Adapter},
    database,
    dex_adapter::DexAdapter,
    flight_recorder::{FlightRecorder, flight_recorder_writer},
    graph_service::GraphService,
    orchestrator::Orchestrator,
    price_feeds::PriceFeed,
    rpc_pool::RpcPool,
    settings::Settings,
    validator::PoolValidator,
    cache::CacheManager,
    hot_pool_manager::HotPoolManager,
    block_number_cache::BlockNumberCache,
    pool_validation_cache::PoolValidationCache,
    weight_refresher,
};
#[cfg(feature = "redis")]
use mig_topology_sdk::route_precomputer::RoutePrecomputer;
#[cfg(feature = "redis")]
use mig_topology_sdk::redis_manager::{self, RedisManager, RedisConfig};
use anyhow::Result;
use ethers::prelude::{Address, Provider, Http};
use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;
use std::fs;
use std::path::Path;
use tokio::signal;
use tokio::time::{interval, Duration};
use std::time::Instant;

/// ✅ REFACTORIZADO: Usa función compartida de hot_pool_manager
/// 
/// Esta función ahora delega a `hot_pool_manager::populate_hot_pool_manager_from_db`
/// para evitar duplicación de código y corregir el bug del fallback.
async fn populate_hot_pool_manager_from_db<M>(
    hot_pool_manager: &HotPoolManager,
    graph_service: &GraphService<M>,
    db_pool: &database::DbPool,
    rpc_pool: Arc<RpcPool>,
) -> Result<usize>
where
    M: ethers::prelude::Middleware + 'static,
{
    use mig_topology_sdk::hot_pool_manager;
    
    hot_pool_manager::populate_hot_pool_manager_from_db(
        hot_pool_manager,
        graph_service,
        db_pool,
        rpc_pool,
        10_000.0,  // min_weight: $10K USD
        200,       // limit: top 200 candidatos
        50,        // max_hot_pools: top 50 pools
        true,      // enable_fallback_refresh: sí, ejecutar full refresh si no hay candidatos
    ).await
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv::dotenv().ok();
    
    env_logger::init();

    println!("🚀 Starting Background Discoverer Service");
    println!("═══════════════════════════════════════════════════════════════════\n");

    // 1. Load settings
    let settings = Settings::new()?;
    println!("✅ Settings loaded");

    // 2. Create RPC pool
    let rpc_pool = Arc::new(RpcPool::new(Arc::new(settings.clone()))?);
    println!("✅ RPC pool created");

    // 3. Connect to database
    let db_pool = database::connect().await?;
    println!("✅ Database connected");

    // 4. Get provider for price feed
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    // provider ya es Arc<Provider<Http>>, usarlo directamente para PriceFeed

    // 5. Create cache manager
    let cache_manager = Arc::new(CacheManager::new());

    // 6. Initialize price feed
    let multicall_address = Address::from_str(&settings.contracts.factories.multicall)?;
    let oracle_addresses: HashMap<Address, Address> = settings
        .price_feeds
        .chainlink_oracles
        .iter()
        .filter_map(|(token, oracle)| {
            Address::from_str(token).ok().and_then(|t| {
                Address::from_str(oracle).ok().map(|o| (t, o))
            })
        })
        .collect();

    let uniswap_v3_factory = Address::from_str(&settings.contracts.factories.uniswap_v3)?;
    let anchor_tokens: Vec<Address> = settings
        .validator
        .anchor_tokens
        .iter()
        .filter_map(|s| Address::from_str(s).ok())
        .collect();

    let price_feed = Arc::new(PriceFeed::new(
        provider.clone(),
        oracle_addresses,
        uniswap_v3_factory,
        settings.price_feeds.cache_ttl_seconds,
        multicall_address,
        settings.performance.multicall_batch_size,
        cache_manager.clone(),
        anchor_tokens.clone(),
        settings.price_feeds.enable_twap_fallback,
        settings.price_feeds.price_deviation_tolerance_bps,
    ));
    println!("✅ Price feed initialized");
    
    // ✅ WARM-UP: Initialize SharedPriceCache and warm up base prices
    use mig_topology_sdk::background_price_updater::{SharedPriceCache, BackgroundPriceUpdater, PriceSource};
    let price_cache = Arc::new(SharedPriceCache::new());
    
    // Base tokens to warm up (anchor tokens + common tokens)
    let base_tokens = anchor_tokens.clone();
    
    println!("🔥 Warming up base prices for {} tokens...", base_tokens.len());
    let warmup_start = Instant::now();
    
    // Attempt to fetch base prices with longer timeout
    match price_feed
        .get_usd_prices_batch_with_chainlink_timeout(
            &base_tokens,
            None,
            Duration::from_millis(2000)
        )
        .await
    {
        Ok(prices) => {
            let valid_count = prices.values().filter(|&&p| p > 0.0).count();
            if valid_count < base_tokens.len() / 2 {
                warn!("⚠️ Only {} out of {} base prices available after warm-up", valid_count, base_tokens.len());
            } else {
                // Update SharedPriceCache with valid prices
                let valid_prices: HashMap<Address, f64> = prices.into_iter()
                    .filter(|(_, p)| *p > 0.0)
                    .collect();
                if !valid_prices.is_empty() {
                    price_cache.update_batch(valid_prices.clone(), PriceSource::Chainlink);
                    println!("✅ Warm-up complete: {} valid prices in {:?}", valid_count, warmup_start.elapsed());
                    for (token, price) in valid_prices.iter().take(5) {
                        println!("  {} = ${:.2}", token, price);
                    }
                }
            }
        }
        Err(e) => {
            warn!("⚠️ Warm-up failed: {}. Continuing anyway...", e);
        }
    }
    
    // Initialize BackgroundPriceUpdater to keep prices fresh
    let critical_tokens = base_tokens.clone();
    let background_updater = Arc::new(BackgroundPriceUpdater::new(
        price_cache.clone(),
        price_feed.clone(),
        critical_tokens,
        5, // Update every 5 seconds
    ));
    
    // Start background updater in background task
    let updater_clone = background_updater.clone();
    tokio::spawn(async move {
        updater_clone.start().await;
    });
    println!("✅ Background price updater started");

    // 7. Create DEX adapters
    let mut adapters: Vec<Box<dyn DexAdapter>> = Vec::new();
    if let Ok(factory) = Address::from_str(&settings.contracts.factories.uniswap_v2) {
        adapters.push(Box::new(UniswapV2Adapter::new(
            factory,
            multicall_address,
            settings.performance.multicall_batch_size,
            rpc_pool.clone(),
        )));
    }
    if let Ok(factory) = Address::from_str(&settings.contracts.factories.uniswap_v3) {
        adapters.push(Box::new(UniswapV3Adapter::new(
            factory,
            multicall_address,
            settings.performance.multicall_batch_size,
            rpc_pool.clone(),
        )));
    }
    println!("✅ DEX adapters created ({} adapters)", adapters.len());

    // 8. Create pool validator
    let validator = Arc::new(PoolValidator::new(
        rpc_pool.clone(),
        &settings.validator,
    ));
    println!("✅ Pool validator created");

    // 8.5. Initialize Flight Recorder for metrics collection
    let (flight_recorder, event_rx) = FlightRecorder::new();
    
    // Create logs directory if it doesn't exist
    let logs_dir = Path::new("logs");
    if !logs_dir.exists() {
        fs::create_dir_all(logs_dir)?;
        println!("✅ Created logs directory");
    }
    
    // Generate output filename with timestamp
    use chrono::Utc;
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let output_file = format!("logs/flight_recorder_{}.jsonl", timestamp);
    let output_file_clone = output_file.clone();
    
    // ✅ CRITICAL: Spawn writer task BEFORE enabling recorder
    tokio::spawn(async move {
        match flight_recorder_writer(event_rx, output_file_clone.clone()).await {
            Ok(_) => println!("✅ Flight Recorder writer completed successfully"),
            Err(e) => eprintln!("❌ Flight Recorder writer error: {}", e),
        }
    });
    
    // Enable Flight Recorder
    flight_recorder.enable();
    let flight_recorder_arc = Arc::new(flight_recorder);
    println!("✅ Flight Recorder initialized and enabled");

    // 9. Create BlockNumberCache for RPC optimization (moved before orchestrator)
    let (provider_for_cache, _permit, endpoint) = rpc_pool.get_next_provider_with_endpoint().await?;
    let block_number_cache = Arc::new(
        BlockNumberCache::new(
            provider_for_cache,
            Duration::from_secs(1), // Update interval: 1 second
        )
        .with_flight_recorder(Some(flight_recorder_arc.clone()), endpoint)
    );
    println!("✅ BlockNumberCache initialized");

    // 10. Create orchestrator with Flight Recorder and BlockNumberCache
    let orchestrator = Orchestrator::new(
        adapters,
        validator,
        db_pool.clone(),
        settings.clone(),
        rpc_pool.clone(),
        price_feed.clone(),
        cache_manager.clone(),
    )?
    .with_flight_recorder(flight_recorder_arc.clone())
    .with_block_number_cache(block_number_cache.clone());
    println!("✅ Orchestrator created");

    // 11. Initialize Redis Manager (optional, requires redis feature)
    // Uses default redis://localhost:6379 if REDIS_URL not set (connects to Docker container)
    #[cfg(feature = "redis")]
    let redis_manager: Option<Arc<tokio::sync::Mutex<redis_manager::RedisManager>>> = {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());
        
        match redis_manager::RedisManager::new(redis_manager::RedisConfig {
            url: redis_url.clone(),
            pool_state_ttl: 10,
            route_cache_ttl: 60,
        }).await {
            Ok(manager) => {
                println!("✅ Redis Manager initialized (connecting to {})", redis_url);
                Some(Arc::new(tokio::sync::Mutex::new(manager)))
            }
            Err(e) => {
                println!("⚠️  Redis Manager initialization failed ({}): {} (continuing without Redis)", redis_url, e);
                None
            }
        }
    };
    #[cfg(not(feature = "redis"))]
    let redis_manager: Option<Arc<tokio::sync::Mutex<redis_manager::RedisManager>>> = None;

    // 12. Create Hot Pool Manager (top-K pools with adaptive refresh)
    let hot_pool_manager = Arc::new(HotPoolManager::new(
        &settings.performance,
        rpc_pool.clone(),
        10000.0, // hot_threshold: $10k USD
    ));
    println!("✅ Hot Pool Manager initialized");

    // 13. Initialize graph service with Hot Pool Manager, BlockNumberCache, and SharedPriceCache
    let graph_service = Arc::new(
        GraphService::new(
            rpc_pool.clone(),
            price_feed.clone(),
            db_pool.clone(),
            multicall_address,
            Arc::new(settings.clone()),
        )
        .await?
        .with_hot_pool_manager(hot_pool_manager.clone())
        .with_block_number_cache(block_number_cache.clone())
        .with_shared_price_cache(price_cache.clone())
    );
    println!("✅ Graph service initialized with SharedPriceCache");

    // ✅ MEJORA: Corregir flags is_active basándose en weights existentes
    println!("🔄 Correcting pool activity flags based on weights...");
    match database::check_pools_activity_improved(&db_pool, 30, 10_000.0).await {
        Ok((activated, deactivated)) => {
            println!("✅ Pool activity flags corrected: {} activated, {} deactivated", activated, deactivated);
        }
        Err(e) => {
            eprintln!("⚠️ Failed to correct pool activity flags: {} (continuing anyway)", e);
        }
    }

    // ✅ ESTRATEGIA HÍBRIDA: Hot refresh inmediato al inicio (pools críticos frescos)
    // Esto asegura que los top 50 pools más importantes tengan weights frescos
    // sin bloquear el startup por 5-10 minutos como haría un full refresh
    println!("🔥 Starting initial hot pools refresh (top 50 pools, weight >= $100K)...");
    match weight_refresher::refresh_hot_pools(
        &graph_service,
        &db_pool,
        rpc_pool.clone(),
        50,        // top 50 pools
        100_000.0, // min weight: $100K
        Some(flight_recorder_arc.clone()),
    ).await {
        Ok(count) => {
            println!("✅ Initial hot pools refresh completed: {} pools updated", count);
        }
        Err(e) => {
            eprintln!("⚠️ Initial hot pools refresh failed: {} (continuing with existing weights)", e);
        }
    }

    // ✅ MEJORADO: Poblar Hot Pool Manager (ahora con weights frescos del hot refresh)
    // El fallback a full refresh solo se ejecuta si realmente no hay candidatos
    println!("🔄 Populating Hot Pool Manager from database...");
    match populate_hot_pool_manager_from_db(
        &hot_pool_manager,
        &*graph_service,
        &db_pool,
        rpc_pool.clone(),
    ).await {
        Ok(count) => {
            println!("✅ Hot Pool Manager populated with {} pools", count);
        }
        Err(e) => {
            eprintln!("❌ Failed to populate Hot Pool Manager: {} (continuing anyway)", e);
        }
    }

    // 14. Initialize PoolValidationCache for RoutePrecomputer
    let pool_validation_cache = Arc::new(PoolValidationCache::new_default());
    println!("✅ PoolValidationCache initialized");

    // 15. Initialize RoutePrecomputer (requires Redis)
    #[cfg(feature = "redis")]
    let route_precomputer: Option<Arc<tokio::sync::Mutex<RoutePrecomputer>>> = {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());
        
        match RoutePrecomputer::new(
            &redis_url,
            db_pool.clone(),
            3600, // cache_ttl_seconds: 1 hour
            pool_validation_cache.clone(),
        ).await {
            Ok(mut precomputer) => {
                println!("✅ RoutePrecomputer initialized (connecting to {})", redis_url);
                Some(Arc::new(tokio::sync::Mutex::new(precomputer)))
            }
            Err(e) => {
                println!("⚠️  RoutePrecomputer initialization failed ({}): {} (continuing without route pre-computation)", redis_url, e);
                None
            }
        }
    };
    #[cfg(not(feature = "redis"))]
    let route_precomputer: Option<Arc<tokio::sync::Mutex<RoutePrecomputer>>> = None;

    // Get intervals from configuration
    let discovery_interval = settings.discovery.interval_seconds;
    let graph_update_interval = settings.graph.update_interval_seconds;
    let route_precompute_interval = 300; // Pre-compute routes every 5 minutes

    println!("\n📊 Service Configuration:");
    println!("   Discovery interval: {} seconds", discovery_interval);
    println!("   Graph update interval: {} seconds", graph_update_interval);
    println!("   Route pre-computation interval: {} seconds", route_precompute_interval);
    println!("\n🔄 Starting background tasks...\n");

    // 16. Spawn discovery task
    let orchestrator_clone = Arc::new(orchestrator);
    let discovery_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(discovery_interval));
        loop {
            interval.tick().await;
            println!("🔍 Running discovery cycle...");
            match orchestrator_clone.run_discovery_cycle().await {
                Ok(_) => println!("✅ Discovery cycle completed"),
                Err(e) => eprintln!("❌ Discovery cycle failed: {}", e),
            }
        }
    });

    // 17. Spawn graph update task
    let graph_service_clone = Arc::clone(&graph_service);
    let hot_pool_manager_clone = Arc::clone(&hot_pool_manager);
    let db_pool_clone = db_pool.clone();
    let rpc_pool_clone = Arc::clone(&rpc_pool);
    
    let graph_update_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(graph_update_interval));
        loop {
            interval.tick().await;
            println!("🔄 Updating graph weights...");
            
            match graph_service_clone.calculate_and_update_all_weights().await {
                Ok(_) => {
                    println!("✅ Graph weights updated");
                    
                    // ✅ Poblar Hot Pool Manager desde BD (usa pesos recién calculados)
                    println!("🔄 Populating Hot Pool Manager from database...");
                    match populate_hot_pool_manager_from_db(
                        &hot_pool_manager_clone,
                        &*graph_service_clone,
                        &db_pool_clone,
                        rpc_pool_clone.clone(),
                    ).await {
                        Ok(count) => {
                            println!("✅ Hot Pool Manager populated with {} pools", count);
                        }
                        Err(e) => {
                            eprintln!("❌ Failed to populate Hot Pool Manager: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ Graph weights update failed: {}", e);
                    
                    // ✅ RESILIENCIA: Intentar poblar de todas formas con pesos antiguos
                    println!("⚠️ Attempting to populate Hot Pool Manager with existing weights...");
                    if let Ok(count) = populate_hot_pool_manager_from_db(
                        &hot_pool_manager_clone,
                        graph_service_clone.as_ref(),
                        &db_pool_clone,
                        rpc_pool_clone.clone(),
                    ).await {
                        println!("✅ Hot Pool Manager populated with {} pools (using stale weights)", count);
                    }
                }
            }
        }
    });

    // 18. Spawn route pre-computation task (if RoutePrecomputer is available)
    let route_precompute_handle = if let Some(ref route_precomputer) = route_precomputer {
        let route_precomputer_clone = Arc::clone(route_precomputer);
        let db_pool_clone = db_pool.clone();
        let block_number_cache_clone = block_number_cache.clone();
        Some(tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(route_precompute_interval));
            loop {
                interval.tick().await;
                println!("🔄 Pre-computing triangular routes...");
                
                // Load active pools from database
                match database::load_active_pools(&db_pool_clone).await {
                    Ok(pools) => {
                        if pools.is_empty() {
                            println!("⚠️  No active pools found for route pre-computation");
                            continue;
                        }
                        
                        // Get current block number
                        let current_block = block_number_cache_clone.get_current_block().await.unwrap_or_else(|_| {
                            tracing::warn!("BlockNumberCache failed, using 0");
                            0u64
                        });
                        
                        if current_block == 0 {
                            println!("⚠️  Could not get current block number, skipping route pre-computation");
                            continue;
                        }
                        
                        // Pre-compute top 1000 routes
                        let mut precomputer = route_precomputer_clone.lock().await;
                        match precomputer.precompute_all_triangular_routes(&pools, current_block, Some(1000)).await {
                            Ok(count) => println!("✅ Pre-computed {} triangular routes", count),
                            Err(e) => eprintln!("❌ Route pre-computation failed: {}", e),
                        }
                    }
                    Err(e) => eprintln!("❌ Failed to load active pools for route pre-computation: {}", e),
                }
            }
        }))
    } else {
        None
    };

    // 19. Spawn hot pools refresh task (every 30 minutes)
    let hot_refresh_handle = {
        let graph_service_clone = Arc::clone(&graph_service);
        let db_pool_clone = db_pool.clone();
        let rpc_pool_clone = Arc::clone(&rpc_pool);
        let flight_recorder_clone = Arc::clone(&flight_recorder_arc);
        
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(30 * 60)); // 30 minutes
            let mut consecutive_failures = 0;
            
            loop {
                interval.tick().await;
                tracing::info!("🔥 Starting hot pools refresh...");
                
                match weight_refresher::refresh_hot_pools(
                    graph_service_clone.as_ref(),
                    &db_pool_clone,
                    rpc_pool_clone.clone(),
                    50,        // top 50 pools
                    100_000.0, // min weight: $100K
                    Some(flight_recorder_clone.clone()), // ✅ FLIGHT RECORDER: Pasar recorder
                ).await {
                    Ok(count) => {
                        consecutive_failures = 0;
                        tracing::info!("✅ Hot pools refresh completed: {} pools updated", count);
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        tracing::error!("❌ Hot pools refresh failed (attempt {}/3): {}", consecutive_failures, e);
                        
                        // Backoff: wait 1 hour if 3 consecutive failures
                        if consecutive_failures >= 3 {
                            tracing::warn!("⚠️ Too many failures, waiting 1 hour before retry...");
                            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
                            consecutive_failures = 0;
                        }
                    }
                }
            }
        })
    };

    // 20. Spawn warm pools refresh task (every 1 hour)
    let warm_refresh_handle = {
        let graph_service_clone = Arc::clone(&graph_service);
        let db_pool_clone = db_pool.clone();
        let rpc_pool_clone = Arc::clone(&rpc_pool);
        let flight_recorder_clone = Arc::clone(&flight_recorder_arc);
        
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60 * 60)); // 1 hour
            let mut consecutive_failures = 0;
            
            loop {
                interval.tick().await;
                tracing::info!("🌡️ Starting warm pools refresh...");
                
                match weight_refresher::refresh_warm_pools(
                    graph_service_clone.as_ref(),
                    &db_pool_clone,
                    rpc_pool_clone.clone(),
                    10_000.0,  // min weight: $10K
                    100_000.0, // max weight: $100K
                    150,       // limit: 150 pools
                    Some(flight_recorder_clone.clone()), // ✅ FLIGHT RECORDER: Pasar recorder
                ).await {
                    Ok(count) => {
                        consecutive_failures = 0;
                        tracing::info!("✅ Warm pools refresh completed: {} pools updated", count);
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        tracing::error!("❌ Warm pools refresh failed (attempt {}/3): {}", consecutive_failures, e);
                        
                        if consecutive_failures >= 3 {
                            tracing::warn!("⚠️ Too many failures, waiting 2 hours before retry...");
                            tokio::time::sleep(Duration::from_secs(2 * 60 * 60)).await;
                            consecutive_failures = 0;
                        }
                    }
                }
            }
        })
    };

    // 21. Spawn daily full refresh task (every 24 hours at 3 AM UTC)
    let full_refresh_handle = {
        let graph_service_clone = Arc::clone(&graph_service);
        let hot_pool_manager_clone = Arc::clone(&hot_pool_manager);
        let db_pool_clone = db_pool.clone();
        let rpc_pool_clone = Arc::clone(&rpc_pool);
        // ✅ FLIGHT RECORDER: Full refresh ya está capturado por GraphService::calculate_and_update_all_weights()
        // que registra eventos "graph_updates" con mode="full"
        
        tokio::spawn(async move {
            loop {
                // Calculate time until next 3 AM UTC
                use chrono::{Utc, Duration as ChronoDuration};
                let now = Utc::now();
                let mut next_3am = now.date_naive()
                    .and_hms_opt(3, 0, 0)
                    .unwrap()
                    .and_utc();
                
                // If it's already past 3 AM today, schedule for tomorrow
                if next_3am <= now {
                    next_3am = next_3am + ChronoDuration::days(1);
                }
                
                let sleep_duration = (next_3am - now).to_std()
                    .unwrap_or(Duration::from_secs(24 * 60 * 60));
                
                tracing::info!("🌍 Next full refresh scheduled for: {} (in {:?})", next_3am, sleep_duration);
                tokio::time::sleep(sleep_duration).await;
                
                tracing::info!("🌍 Starting daily full refresh...");
                // ✅ FLIGHT RECORDER: calculate_and_update_all_weights() ya registra eventos
                match graph_service_clone.calculate_and_update_all_weights().await {
                    Ok(_) => {
                        tracing::info!("✅ Daily full refresh completed");
                        
                        // Repopulate Hot Pool Manager with fresh weights
                        match populate_hot_pool_manager_from_db(
                            &hot_pool_manager_clone,
                            &*graph_service_clone,
                            &db_pool_clone,
                            rpc_pool_clone.clone(),
                        ).await {
                            Ok(count) => {
                                tracing::info!("✅ Hot Pool Manager repopulated with {} pools after full refresh", count);
                            }
                            Err(e) => {
                                tracing::error!("❌ Failed to repopulate Hot Pool Manager: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("❌ Daily full refresh failed: {}", e);
                    }
                }
            }
        })
    };

    // 22. Wait for shutdown signal
    println!("💡 Service running:");
    println!("   - Discovery cycles run every {} seconds", discovery_interval);
    println!("   - Graph weight updates run every {} seconds", graph_update_interval);
    if route_precomputer.is_some() {
        println!("   - Route pre-computation runs every {} seconds", route_precompute_interval);
    }
    println!("\nPress Ctrl+C to stop gracefully...\n");

    signal::ctrl_c().await?;
    println!("\n🛑 Shutdown signal received, stopping tasks...");
    
    // Cancel all tasks
    discovery_handle.abort();
    graph_update_handle.abort();
    hot_refresh_handle.abort();
    warm_refresh_handle.abort();
    full_refresh_handle.abort();
    if let Some(handle) = route_precompute_handle {
        handle.abort();
    }

    println!("✅ Shutdown complete");

    Ok(())
}
