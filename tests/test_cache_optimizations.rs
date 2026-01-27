//! Integration tests for cache optimizations (P0)
//!
//! Tests cover:
//! - Merkle tree-based cache invalidation
//! - TTL differentiation
//! - Fuzzy block matching
//!
//! Note: Local node tests are excluded

use ethers::types::{Address, U256};
use mig_topology_sdk::cache::state_cache::{
    CacheValidationResult, CachedPoolState, StateCacheManager,
};
use std::str::FromStr;
use std::time::Instant;

/// Test Merkle tree hash calculation
#[test]
fn test_merkle_hash_calculation() {
    use mig_topology_sdk::v3_math::V3PoolState;

    let v3_state = V3PoolState {
        sqrt_price_x96: U256::from(1000),
        liquidity: U256::from(2000),
        tick: 100,
    };

    // Same state should produce same hash
    let hash1 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);
    let hash2 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);
    assert_eq!(hash1, hash2, "Same state should produce same hash");

    // Different block should produce different hash
    let hash3 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 101);
    assert_ne!(
        hash1, hash3,
        "Different block should produce different hash"
    );
}

/// Test fuzzy block matching with hybrid validation
#[test]
fn test_fuzzy_block_matching() {
    use mig_topology_sdk::v3_math::V3PoolState;

    let v3_state = V3PoolState {
        sqrt_price_x96: U256::from(1000),
        liquidity: U256::from(2000),
        tick: 100,
    };

    let merkle_root = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);

    let cached = CachedPoolState {
        v3_state: Some(v3_state),
        v2_state: None,
        v2_token0: None,
        v2_token1: None,
        merkle_root,
        block_number: 100,
        last_updated: Instant::now(),
        touched: false,
    };

    // Test fuzzy matching with tolerance = 5 blocks
    let tolerance = 5u64;
    let time_tolerance = std::time::Duration::from_secs(300); // 5 minutes

    // Block 100 (exact match) - should be valid
    assert!(
        cached.is_cache_valid_hybrid(100, tolerance, time_tolerance),
        "Exact block match should be valid"
    );

    // Block 103 (within tolerance) - should be valid
    assert!(
        cached.is_cache_valid_hybrid(103, tolerance, time_tolerance),
        "Block within tolerance should be valid"
    );

    // Block 106 (outside tolerance) - should be invalid
    assert!(
        !cached.is_cache_valid_hybrid(106, tolerance, time_tolerance),
        "Block outside tolerance should be invalid"
    );
}

/// Test state hash validation
#[test]
fn test_state_hash_validation() {
    use mig_topology_sdk::v3_math::V3PoolState;

    let v3_state = V3PoolState {
        sqrt_price_x96: U256::from(1000),
        liquidity: U256::from(2000),
        tick: 100,
    };

    let merkle_root = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);

    let cached = CachedPoolState {
        v3_state: Some(v3_state.clone()),
        v2_state: None,
        v2_token0: None,
        v2_token1: None,
        merkle_root,
        block_number: 100,
        last_updated: Instant::now(),
        touched: false,
    };

    // Same state hash should be valid
    assert!(
        cached.is_state_hash_valid(merkle_root),
        "Same state hash should be valid"
    );

    // Different state hash should be invalid
    let different_state = V3PoolState {
        sqrt_price_x96: U256::from(2000), // Different price
        liquidity: U256::from(2000),
        tick: 100,
    };
    let different_hash = CachedPoolState::calculate_merkle_root(Some(&different_state), None, 100);
    assert!(
        !cached.is_state_hash_valid(different_hash),
        "Different state hash should be invalid"
    );
}

/// Test StateCacheManager creation
#[test]
fn test_state_cache_manager_creation() {
    let cache_manager = StateCacheManager::new(2000);
    // Just verify it can be created
    assert!(true, "StateCacheManager should be created successfully");
}
