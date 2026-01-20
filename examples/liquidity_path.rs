//! # Liquidity Path Example
//!
//! This example demonstrates how to find the deepest liquidity path between two tokens
//! using the MIG Topology SDK's graph service.
//!
//! ## Overview
//!
//! The example:
//! 1. Initializes the SDK (see `basic_setup.rs` for details)
//! 2. Loads the graph weights from the database
//! 3. Finds pools connecting WETH to USDC
//! 4. Identifies the pool with the highest liquidity weight
//!
//! ## Prerequisites
//!
//! - Database must be initialized and contain discovered pools
//! - At least one pool connecting the two tokens must exist
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example liquidity_path -- WETH_ADDRESS USDC_ADDRESS
//! ```

use mig_topology_sdk::{
    database,
    graph_service::GraphService,
    price_feeds::PriceFeed,
    rpc_pool::RpcPool,
    settings::Settings,
    cache::CacheManager,
    block_number_cache::BlockNumberCache,
};
use anyhow::{Context, Result};
use ethers::prelude::{Address, Provider, Http};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} TOKEN0_ADDRESS TOKEN1_ADDRESS", args[0]);
        eprintln!("Example: {} 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1 0xaf88d065e77c8cC2239327C5EDb3A432268e5831", args[0]);
        eprintln!("        (WETH on Arbitrum -> USDC on Arbitrum)");
        std::process::exit(1);
    }

    let token0 = Address::from_str(&args[1])
        .context("Invalid TOKEN0_ADDRESS")?;
    let token1 = Address::from_str(&args[2])
        .context("Invalid TOKEN1_ADDRESS")?;

    println!("🔍 Finding liquidity path from {:?} to {:?}", token0, token1);

    // Initialize SDK components (simplified - see basic_setup.rs for full initialization)
    let settings = Settings::new()?;
    let rpc_pool = Arc::new(RpcPool::new(Arc::new(settings.clone()))?);
    let db_pool = database::connect().await?;

    // Initialize price feed
    let (provider, _permit) = rpc_pool.get_next_provider().await?;
    let provider = Arc::new(provider);
    let multicall_address = Address::from_str(&settings.contracts.factories.multicall)?;

    let oracle_addresses: HashMap<Address, Address> = HashMap::new(); // Simplified
    let uniswap_v3_factory = Address::from_str(&settings.contracts.factories.uniswap_v3)?;
    let cache_manager = Arc::new(CacheManager::new());
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

    // Create BlockNumberCache for RPC optimization (optional)
    let (provider_for_cache, _permit, endpoint) = rpc_pool.get_next_provider_with_endpoint().await?;
    let block_number_cache = Arc::new(
        BlockNumberCache::new(
            provider_for_cache,
            std::time::Duration::from_secs(1), // Update interval: 1 second
        )
        .with_flight_recorder(None, endpoint) // No flight recorder in liquidity path example
    );

    // Initialize graph service (no longer requires JitStateFetcher)
    let graph_service = GraphService::new(
        rpc_pool.clone(),
        price_feed.clone(),
        db_pool.clone(),
        multicall_address,
        Arc::new(settings.clone()),
    )
    .await?
    .with_block_number_cache(block_number_cache);

    // Query database for pools connecting the two tokens
    println!("\n📊 Querying database for pools...");
    
    // Load pools from database (simplified - in production, use proper query)
    let pools = database::load_active_pools(&db_pool).await?;
    
    // Filter pools that connect token0 and token1
    let connecting_pools: Vec<_> = pools
        .iter()
        .filter(|pool| {
            let (t0, t1) = match pool {
                mig_topology_sdk::pools::Pool::UniswapV2(p) => (p.token0, p.token1),
                mig_topology_sdk::pools::Pool::UniswapV3(p) => (p.token0, p.token1),
                mig_topology_sdk::pools::Pool::BalancerWeighted(p) => {
                    (p.tokens.get(0).copied().unwrap_or_default(),
                     p.tokens.get(1).copied().unwrap_or_default())
                }
                mig_topology_sdk::pools::Pool::CurveStableSwap(p) => {
                    (p.tokens.get(0).copied().unwrap_or_default(),
                     p.tokens.get(1).copied().unwrap_or_default())
                }
            };
            (t0 == token0 && t1 == token1) || (t0 == token1 && t1 == token0)
        })
        .collect();

    if connecting_pools.is_empty() {
        println!("❌ No pools found connecting {:?} to {:?}", token0, token1);
        println!("   Make sure you've run discovery first!");
        return Ok(());
    }

    println!("✅ Found {} pools connecting the tokens", connecting_pools.len());

    // Find pool with highest weight
    let mut best_pool: Option<(&mig_topology_sdk::pools::Pool, f64)> = None;

    for pool in &connecting_pools {
        let pool_address = pool.address();
        if let Some(weight) = graph_service.get_weight(&pool_address) {
            if weight > 0.0 {
                match best_pool {
                    None => best_pool = Some((pool, weight)),
                    Some((_, best_weight)) if weight > best_weight => {
                        best_pool = Some((pool, weight))
                    }
                    _ => {}
                }
            }
        }
    }

    match best_pool {
        Some((pool, weight)) => {
            println!("\n🏆 Best liquidity path found:");
            println!("   Pool: {:?}", pool.address());
            println!("   DEX: {}", pool.dex());
            println!("   Weight (USD): ${:.2}", weight);
            
            let (t0, t1) = match pool {
                mig_topology_sdk::pools::Pool::UniswapV2(p) => (p.token0, p.token1),
                mig_topology_sdk::pools::Pool::UniswapV3(p) => (p.token0, p.token1),
                mig_topology_sdk::pools::Pool::BalancerWeighted(p) => {
                    (p.tokens.get(0).copied().unwrap_or_default(),
                     p.tokens.get(1).copied().unwrap_or_default())
                }
                mig_topology_sdk::pools::Pool::CurveStableSwap(p) => {
                    (p.tokens.get(0).copied().unwrap_or_default(),
                     p.tokens.get(1).copied().unwrap_or_default())
                }
            };
            
            println!("   Token0: {:?}", t0);
            println!("   Token1: {:?}", t1);
        }
        None => {
            println!("⚠️  No pools with valid weights found");
            println!("   Try running: graph_service.calculate_and_update_all_weights().await?");
        }
    }

    Ok(())
}

