//! # DEX Adapter Trait
//!
//! This module defines the core abstraction for integrating different DEX protocols
//! into the MIG Topology SDK. The `DexAdapter` trait provides a unified interface
//! for pool discovery and state fetching across all supported protocols.
//!
//! ## Overview
//!
//! The adapter pattern allows the SDK to support multiple DEX protocols (Uniswap V2/V3,
//! Balancer, Curve, etc.) without modifying core discovery logic. Each protocol implements
//! the `DexAdapter` trait to provide protocol-specific discovery and state fetching.
//!
//! ## Adding a New DEX Protocol
//!
//! To add support for a new DEX protocol:
//!
//! 1. Implement the `DexAdapter` trait for your protocol
//! 2. Add a `Pool` enum variant if needed (see `pools.rs`)
//! 3. Register the adapter in the `Orchestrator`
//!
//! See `docs/ARCHITECTURE.md` for detailed implementation guide.
//!
//! ## Example
//!
//! ```rust,no_run
//! use mig_topology_sdk::dex_adapter::{DexAdapter, PoolMeta};
//! use async_trait::async_trait;
//!
//! struct MyDexAdapter {
//!     factory: Address,
//! }
//!
//! #[async_trait]
//! impl DexAdapter for MyDexAdapter {
//!     fn name(&self) -> &'static str {
//!         "MyDex"
//!     }
//!
//!     async fn discover_pools(
//!         &self,
//!         from_block: u64,
//!         to_block: u64,
//!         chunk_size: u64,
//!         max_concurrency: usize,
//!     ) -> Result<Vec<PoolMeta>> {
//!         // Query factory events and convert to PoolMeta
//!         // ...
//!     }
//!
//!     async fn fetch_pool_state(&self, pools: &[PoolMeta]) -> Result<Vec<Pool>> {
//!         // Fetch pool reserves/state and convert to Pool enum
//!         // ...
//!     }
//! }
//! ```

use anyhow::Result;
use async_trait::async_trait;
use ethers::prelude::*;

/// Represents the static metadata of a liquidity pool.
///
/// This structure provides a protocol-agnostic representation of pool metadata,
/// allowing the SDK to work with pools from different DEX protocols uniformly.
///
/// # Fields
///
/// - `address`: The contract address of the pool
/// - `factory`: The factory contract that created this pool (if applicable)
/// - `pool_id`: Protocol-specific pool identifier (e.g., Balancer pool ID)
/// - `fee`: Fee tier in basis points (e.g., 3000 for 0.3%, only for V3-style pools)
/// - `token0`: First token in the pair
/// - `token1`: Second token in the pair
/// - `dex`: Name of the DEX protocol (e.g., "Uniswap V3", "Balancer")
/// - `pool_type`: Protocol-specific pool type (e.g., "Weighted" for Balancer)
#[derive(Debug, Clone)]
pub struct PoolMeta {
    /// The contract address of the pool
    pub address: Address,
    /// The factory that created the pool (None for registry-based protocols like Curve)
    pub factory: Option<Address>,
    /// Optional pool ID (used by Balancer and some other protocols)
    pub pool_id: Option<[u8; 32]>,
    /// Fee tier in basis points (e.g., 3000 = 0.3%). None for fixed-fee protocols like Uniswap V2
    pub fee: Option<u32>,
    /// First token in the pair
    pub token0: Address,
    /// Second token in the pair
    pub token1: Address,
    /// Name of the DEX protocol
    pub dex: &'static str,
    /// Protocol-specific pool type (e.g., "Weighted", "StableSwap")
    pub pool_type: Option<String>,
}

use crate::pools::Pool;

impl From<&Pool> for PoolMeta {
    fn from(pool: &Pool) -> Self {
        let (token0, token1) = match pool {
            Pool::UniswapV2(p) => (p.token0, p.token1),
            Pool::UniswapV3(p) => (p.token0, p.token1),
            Pool::BalancerWeighted(p) => (p.tokens.get(0).cloned().unwrap_or_default(), p.tokens.get(1).cloned().unwrap_or_default()),
            Pool::CurveStableSwap(p) => (p.tokens.get(0).cloned().unwrap_or_default(), p.tokens.get(1).cloned().unwrap_or_default()),
        };

        let fee = match pool {
            Pool::UniswapV3(p) => Some(p.fee),
            _ => None,
        };

        let pool_id = match pool {
            Pool::BalancerWeighted(p) => Some(p.pool_id),
            _ => None,
        };

        Self {
            address: pool.address(),
            factory: None, // This info is lost in the Pool struct, but not critical for upsert
            pool_id,
            fee,
            token0,
            token1,
            dex: pool.dex(),
            pool_type: None, // Also lost, not critical
        }
    }
}

/// The main trait for all DEX protocol adapters.
///
/// This trait defines the interface that all DEX protocol implementations must provide.
/// It enables the SDK to discover pools and fetch their state in a protocol-agnostic manner.
///
/// # Implementation Requirements
///
/// - `name()`: Return a static string identifying the protocol
/// - `discover_pools()`: Query the protocol's factory/registry for new pools in a block range
/// - `fetch_pool_state()`: Retrieve current state (reserves, liquidity, etc.) for given pools
///
/// # Thread Safety
///
/// All adapters must be `Send + Sync` to allow concurrent discovery across multiple protocols.
///
/// # Example Implementation
///
/// See `src/adapters/uniswap_v2.rs` for a complete example implementation.
#[async_trait]
pub trait DexAdapter: Send + Sync {
    /// Returns the name of the DEX protocol.
    ///
    /// This name is used for logging, metrics, and database storage.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// fn name(&self) -> &'static str {
    ///     "Uniswap V2"
    /// }
    /// ```
    fn name(&self) -> &'static str;

    /// Discovers pools created within the specified block range.
    ///
    /// This method queries the protocol's factory contract or registry for pool creation events
    /// and returns a list of `PoolMeta` structures representing discovered pools.
    ///
    /// # Parameters
    ///
    /// - `from_block`: Starting block number (inclusive)
    /// - `to_block`: Ending block number (inclusive)
    /// - `chunk_size`: Maximum block range per RPC call (for batching)
    /// - `max_concurrency`: Maximum number of concurrent RPC calls
    ///
    /// # Returns
    ///
    /// A `Result` containing a vector of `PoolMeta` structures for discovered pools.
    ///
    /// # Errors
    ///
    /// Returns an error if RPC calls fail or if event parsing fails.
    ///
    /// # Implementation Notes
    ///
    /// - Use `get_logs()` to query factory events (e.g., `PairCreated`, `PoolCreated`)
    /// - Parse event logs to extract pool addresses and metadata
    /// - Handle protocol-specific quirks (e.g., Curve uses static registry, not events)
    /// - Consider using multicall for batch queries when possible
    async fn discover_pools(
        &self,
        from_block: u64,
        to_block: u64,
        chunk_size: u64,
        max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>>;

    /// Fetches the current state for a list of pools.
    ///
    /// This method retrieves runtime state data (reserves, liquidity, prices, etc.) for
    /// the given pools. The state is protocol-specific but is normalized into the `Pool` enum.
    ///
    /// # Parameters
    ///
    /// - `pools`: Slice of `PoolMeta` structures to fetch state for
    ///
    /// # Returns
    ///
    /// A `Result` containing a vector of `Pool` enum variants with current state.
    ///
    /// # Errors
    ///
    /// Returns an error if RPC calls fail or if state parsing fails.
    ///
    /// # Implementation Notes
    ///
    /// - Use multicall batching to fetch multiple pools efficiently
    /// - For Uniswap V2: Call `getReserves()` on pair contracts
    /// - For Uniswap V3: Call `slot0()` and `liquidity()` on pool contracts
    /// - For Balancer: Call `getPool()` on vault contract
    /// - Handle failed calls gracefully (some pools may be invalid)
    async fn fetch_pool_state(&self, pools: &[PoolMeta]) -> Result<Vec<Pool>>;
}
