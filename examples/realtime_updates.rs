//! # Real-Time Graph Updates Example
//!
//! This example demonstrates how to listen for real-time updates to the liquidity graph.
//!
//! ## Overview
//!
//! The example:
//! 1. Initializes the SDK
//! 2. Sets up a periodic task to update graph weights
//! 3. Monitors pool discovery and graph updates
//! 4. Shows how to react to topology changes
//!
//! ## Prerequisites
//!
//! - Database must be initialized
//! - RPC provider must be accessible
//! - Discovery should have run at least once
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example realtime_updates
//! ```
//!
//! Press Ctrl+C to stop.

use anyhow::Result;
use ethers::prelude::{Address, Http, Provider};
use mig_topology_sdk::{
    adapters::{uniswap_v2::UniswapV2Adapter, uniswap_v3::UniswapV3Adapter},
    block_number_cache::BlockNumberCache,
    cache::CacheManager,
    database,
    dex_adapter::DexAdapter,
    graph_service::GraphService,
    orchestrator::Orchestrator,
    price_feeds::PriceFeed,
    rpc_pool::RpcPool,
    settings::Settings,
    validator::PoolValidator,
};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::signal;
use tokio::time::{interval, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    println!("🚀 Starting real-time topology monitoring...");

    // Initialize SDK (see basic_setup.rs for details)
    let settings = Settings::new()?;
    let rpc_pool = Arc::new(RpcPool::new(Arc::new(settings.clone()))?);
    let db_pool = database::connect().await?;

    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    let provider = Arc::new(provider);
    let multicall_address = Address::from_str(&settings.contracts.factories.multicall)?;
    let cache_manager = Arc::new(CacheManager::new());

    let oracle_addresses: HashMap<Address, Address> = HashMap::new();
    let uniswap_v3_factory = Address::from_str(&settings.contracts.factories.uniswap_v3)?;
    let anchor_tokens: Vec<Address> = vec![];

    let price_feed = Arc::new(PriceFeed::new(
        provider.clone(),
        oracle_addresses,
        uniswap_v3_factory,
        settings.price_feeds.cache_ttl_seconds,
        multicall_address,
        settings.performance.multicall_batch_size,
        cache_manager.clone(),
        anchor_tokens,
        settings.price_feeds.enable_twap_fallback,
        settings.price_feeds.price_deviation_tolerance_bps,
    ));

    // Create adapters
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

    let validator = Arc::new(PoolValidator::new(rpc_pool.clone(), &settings.validator));

    let orchestrator = Arc::new(Orchestrator::new(
        adapters,
        validator,
        db_pool.clone(),
        settings.clone(),
        rpc_pool.clone(),
        price_feed.clone(),
        cache_manager.clone(),
    )?);

    // Create BlockNumberCache for RPC optimization (optional)
    let (provider_for_cache, _permit, endpoint) =
        rpc_pool.get_next_provider_with_endpoint().await?;
    let block_number_cache = Arc::new(
        BlockNumberCache::new(
            provider_for_cache,
            Duration::from_secs(1), // Update interval: 1 second
        )
        .with_flight_recorder(None, endpoint), // No flight recorder in realtime updates example
    );

    // Initialize graph service (no longer requires JitStateFetcher)
    let graph_service = Arc::new(
        GraphService::new(
            rpc_pool.clone(),
            price_feed.clone(),
            db_pool.clone(),
            multicall_address,
            Arc::new(settings.clone()),
        )
        .await?
        .with_block_number_cache(block_number_cache),
    );

    println!("✅ SDK initialized");
    println!("📊 Monitoring graph updates (press Ctrl+C to stop)\n");

    // Spawn task for periodic graph weight updates
    let graph_service_clone = Arc::clone(&graph_service);
    let update_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60)); // Update every 60 seconds
        loop {
            interval.tick().await;
            println!("🔄 Updating graph weights...");
            match graph_service_clone.calculate_and_update_all_weights().await {
                Ok(_) => println!("✅ Graph weights updated"),
                Err(e) => eprintln!("❌ Failed to update weights: {}", e),
            }
        }
    });

    // Spawn task for periodic discovery (optional - can be disabled)
    let orchestrator_clone = orchestrator.clone();
    let discovery_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(300)); // Run discovery every 5 minutes
        loop {
            interval.tick().await;
            println!("🔍 Running discovery cycle...");
            match orchestrator_clone.run_discovery_cycle().await {
                Ok(_) => println!("✅ Discovery cycle completed"),
                Err(e) => eprintln!("❌ Discovery failed: {}", e),
            }
        }
    });

    // Monitor graph statistics
    let graph_service_stats = Arc::clone(&graph_service);
    let db_pool_stats = db_pool.clone();
    let stats_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(30)); // Print stats every 30 seconds
        loop {
            interval.tick().await;

            // Get pool count from database
            match database::get_valid_pools_count_per_dex(&db_pool_stats).await {
                Ok(counts) => {
                    println!("\n📊 Graph Statistics:");
                    let total: i64 = counts.values().sum();
                    println!("   Total valid pools: {}", total);
                    for (dex, count) in &counts {
                        println!("   {}: {} pools", dex, count);
                    }

                    // Sample a few pool weights
                    let sample_pools = database::load_active_pools(&db_pool_stats)
                        .await
                        .unwrap_or_default();
                    if !sample_pools.is_empty() {
                        println!("\n   Sample pool weights:");
                        for pool in sample_pools.iter().take(5) {
                            let addr = pool.address();
                            if let Some(weight) = graph_service_stats.get_weight(&addr) {
                                println!("     {:?}: ${:.2}", addr, weight);
                            }
                        }
                    }
                    println!();
                }
                Err(e) => eprintln!("❌ Failed to get stats: {}", e),
            }
        }
    });

    // Wait for shutdown signal
    println!("💡 Tips:");
    println!("   - Graph weights update every 60 seconds");
    println!("   - Discovery runs every 5 minutes");
    println!("   - Statistics print every 30 seconds");
    println!("\nPress Ctrl+C to stop...\n");

    signal::ctrl_c().await?;
    println!("\n🛑 Shutting down...");

    // Cancel tasks
    update_handle.abort();
    discovery_handle.abort();
    stats_handle.abort();

    println!("✅ Shutdown complete");

    Ok(())
}
