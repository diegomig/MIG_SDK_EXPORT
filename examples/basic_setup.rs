//! # Basic SDK Setup Example
//!
//! This example demonstrates how to initialize the MIG Topology SDK with all required components:
//! - Settings configuration
//! - RPC pool setup
//! - Database connection
//! - DEX adapters creation
//! - Pool validator initialization
//!
//! ## Prerequisites
//!
//! - Set `DATABASE_URL` environment variable
//! - Set `RPC_URL` environment variable (or configure in settings)
//! - Ensure PostgreSQL is running and initialized
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example basic_setup
//! ```

use mig_topology_sdk::{
    adapters::{uniswap_v2::UniswapV2Adapter, uniswap_v3::UniswapV3Adapter},
    database,
    dex_adapter::DexAdapter,
    graph_service::GraphService,
    orchestrator::Orchestrator,
    price_feeds::PriceFeed,
    rpc_pool::RpcPool,
    settings::Settings,
    validator::PoolValidator,
    cache::CacheManager,
    block_number_cache::BlockNumberCache,
};
use anyhow::Result;
use ethers::prelude::{Address, Provider, Http};
use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();

    println!("🚀 Initializing MIG Topology SDK...");

    // 1. Load settings from config file or environment
    let settings = Settings::new()?;
    println!("✅ Settings loaded");

    // 2. Create RPC pool for blockchain queries
    let rpc_pool = Arc::new(RpcPool::new(Arc::new(settings.clone()))?);
    println!("✅ RPC pool created");

    // 3. Connect to PostgreSQL database
    let db_pool = database::connect().await?;
    println!("✅ Database connected");

    // 4. Get a provider for price feed initialization
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    let provider = Arc::new(provider);

    // 5. Create cache manager
    let cache_manager = Arc::new(CacheManager::new());
    println!("✅ Cache manager created");

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
        anchor_tokens,
        settings.price_feeds.enable_twap_fallback,
        settings.price_feeds.price_deviation_tolerance_bps,
    ));
    println!("✅ Price feed initialized");

    // 7. Create DEX adapters
    let mut adapters: Vec<Box<dyn DexAdapter>> = Vec::new();

    // Uniswap V2 adapter
    if let Ok(factory) = Address::from_str(&settings.contracts.factories.uniswap_v2) {
        adapters.push(Box::new(UniswapV2Adapter::new(
            factory,
            multicall_address,
            settings.performance.multicall_batch_size,
            rpc_pool.clone(),
        )));
        println!("✅ Uniswap V2 adapter created");
    }

    // Uniswap V3 adapter
    if let Ok(factory) = Address::from_str(&settings.contracts.factories.uniswap_v3) {
        adapters.push(Box::new(UniswapV3Adapter::new(
            factory,
            multicall_address,
            settings.performance.multicall_batch_size,
            rpc_pool.clone(),
        )));
        println!("✅ Uniswap V3 adapter created");
    }

    // 8. Create pool validator
    let validator = Arc::new(PoolValidator::new(
        rpc_pool.clone(),
        &settings.validator,
    ));
    println!("✅ Pool validator created");

    // 9. Create orchestrator
    let orchestrator = Orchestrator::new(
        adapters,
        validator,
        db_pool.clone(),
        settings.clone(),
        rpc_pool.clone(),
        price_feed.clone(),
        cache_manager.clone(),
    )?;
    println!("✅ Orchestrator created");

    // 10. Create BlockNumberCache for RPC optimization (optional)
    let (provider_for_cache, _permit, endpoint) = rpc_pool.get_next_provider_with_endpoint().await?;
    let block_number_cache = Arc::new(
        BlockNumberCache::new(
            provider_for_cache,
            std::time::Duration::from_secs(1), // Update interval: 1 second
        )
        .with_flight_recorder(None, endpoint) // No flight recorder in basic setup
    );
    println!("✅ BlockNumberCache initialized");

    // 11. Initialize graph service (no longer requires JitStateFetcher)
    let graph_service = GraphService::new(
        rpc_pool.clone(),
        price_feed.clone(),
        db_pool.clone(),
        multicall_address,
        Arc::new(settings.clone()),
    )
    .await?
    .with_block_number_cache(block_number_cache);
    println!("✅ Graph service initialized");

    println!("\n🎉 SDK initialization complete!");
    println!("\nYou can now use:");
    println!("  - orchestrator.run_discovery_cycle().await?");
    println!("  - graph_service.get_weight(&pool_address)");
    println!("  - graph_service.calculate_and_update_all_weights().await?");

    Ok(())
}

