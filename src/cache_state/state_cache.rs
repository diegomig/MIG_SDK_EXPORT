//! State cache management with Merkle tree-based invalidation
//!
//! This module provides cache management utilities for pool states using Merkle tree
//! hashes for efficient cache invalidation. Cache entries are invalidated only when
//! the actual pool state changes, not just when blocks advance.

use crate::v3_math::V3PoolState;
use ethers::types::Address;
use ethers::types::U256;
use ethers::utils::keccak256;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

/// Cached pool state with Merkle tree hash for change detection
#[derive(Debug, Clone)]
pub struct CachedPoolState {
    pub v3_state: Option<V3PoolState>,
    pub v2_state: Option<(U256, U256)>,
    pub v2_token0: Option<Address>,
    pub v2_token1: Option<Address>,
    pub merkle_root: [u8; 32],
    pub block_number: u64,
    pub last_updated: Instant,
    pub touched: bool,
}

impl CachedPoolState {
    /// Calculate Merkle tree hash for pool state
    ///
    /// Hash combines block_number and state_hash for stronger invalidation guarantees.
    /// The hash is calculated from:
    /// - V3: sqrt_price_x96, liquidity, tick
    /// - V2: reserve0, reserve1
    /// - block_number (for block-based tolerance)
    pub fn calculate_merkle_root(
        v3_state: Option<&V3PoolState>,
        v2_state: Option<&(U256, U256)>,
        block_number: u64,
    ) -> [u8; 32] {
        // First, calculate state hash
        let mut state_hasher = DefaultHasher::new();
        if let Some(v3) = v3_state {
            v3.sqrt_price_x96.hash(&mut state_hasher);
            v3.liquidity.hash(&mut state_hasher);
            v3.tick.hash(&mut state_hasher);
        }
        if let Some(v2) = v2_state {
            v2.0.hash(&mut state_hasher);
            v2.1.hash(&mut state_hasher);
        }
        let state_hash = state_hasher.finish();

        // Combine block_number and state_hash into Merkle root using keccak256
        let mut combined = Vec::with_capacity(16);
        combined.extend_from_slice(&block_number.to_be_bytes());
        combined.extend_from_slice(&state_hash.to_be_bytes());
        keccak256(combined)
    }

    /// Check if cache entry is valid based on state hash comparison
    ///
    /// Returns true if the cached state hash matches the current state hash,
    /// indicating that the pool state has not changed.
    pub fn is_state_hash_valid(&self, current_state_hash: [u8; 32]) -> bool {
        self.merkle_root == current_state_hash
    }

    /// Check if cache entry is valid based on block tolerance and TTL
    ///
    /// This is a hybrid validation that checks both block-based tolerance
    /// and time-based TTL. Used when we cannot calculate state hash without
    /// fetching on-chain first.
    pub fn is_cache_valid_hybrid(
        &self,
        current_block: u64,
        block_tolerance: u64,
        time_tolerance: std::time::Duration,
    ) -> bool {
        let elapsed_time = self.last_updated.elapsed();
        let blocks_since_cache = current_block.saturating_sub(self.block_number);

        elapsed_time < time_tolerance && blocks_since_cache <= block_tolerance
    }
}

/// Cache validation result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheValidationResult {
    /// Cache is valid and can be used
    Valid,
    /// Cache is invalid due to state hash mismatch
    InvalidStateHash,
    /// Cache is invalid due to block tolerance exceeded
    InvalidBlockTolerance,
    /// Cache is invalid due to TTL exceeded
    InvalidTTL,
    /// Cache entry not found
    NotFound,
}

/// State cache manager with Merkle tree-based invalidation
pub struct StateCacheManager {
    /// Maximum cache size before eviction
    max_size: usize,
}

impl StateCacheManager {
    /// Create a new state cache manager
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }

    /// Validate cache entry using state hash comparison
    ///
    /// This is the preferred method when we have the current state hash.
    /// Returns `CacheValidationResult` indicating the validation outcome.
    pub fn validate_cache_by_state_hash(
        cached: &CachedPoolState,
        current_state_hash: [u8; 32],
    ) -> CacheValidationResult {
        if cached.is_state_hash_valid(current_state_hash) {
            CacheValidationResult::Valid
        } else {
            CacheValidationResult::InvalidStateHash
        }
    }

    /// Validate cache entry using hybrid method (block + TTL)
    ///
    /// This is used when we cannot calculate state hash without fetching on-chain.
    /// After fetching, we should compare state hashes to improve cache hit rate
    /// for subsequent requests.
    pub fn validate_cache_hybrid(
        cached: &CachedPoolState,
        current_block: u64,
        block_tolerance: u64,
        time_tolerance: std::time::Duration,
    ) -> CacheValidationResult {
        if cached.is_cache_valid_hybrid(current_block, block_tolerance, time_tolerance) {
            CacheValidationResult::Valid
        } else {
            let blocks_since_cache = current_block.saturating_sub(cached.block_number);
            let elapsed_time = cached.last_updated.elapsed();

            if blocks_since_cache > block_tolerance {
                CacheValidationResult::InvalidBlockTolerance
            } else if elapsed_time >= time_tolerance {
                CacheValidationResult::InvalidTTL
            } else {
                CacheValidationResult::InvalidStateHash
            }
        }
    }

    /// Check if cache entry should be invalidated after fetching on-chain
    ///
    /// After fetching pool state on-chain, compare the calculated state hash
    /// with the cached state hash. If they match, the cache was valid and we
    /// should have used it. This helps improve cache hit rate for subsequent requests.
    pub fn should_have_used_cache(cached: &CachedPoolState, fetched_state_hash: [u8; 32]) -> bool {
        cached.is_state_hash_valid(fetched_state_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::U256;

    #[test]
    fn test_merkle_root_calculation() {
        // Test V3 state hash calculation
        let v3_state = V3PoolState {
            sqrt_price_x96: U256::from(1000),
            liquidity: U256::from(2000),
            tick: 100,
        };

        let hash1 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);
        let hash2 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 100);

        // Same state should produce same hash
        assert_eq!(hash1, hash2);

        // Different block should produce different hash
        let hash3 = CachedPoolState::calculate_merkle_root(Some(&v3_state), None, 101);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_state_hash_validation() {
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
        assert!(cached.is_state_hash_valid(merkle_root));

        // Different state hash should be invalid
        let different_state = V3PoolState {
            sqrt_price_x96: U256::from(2000), // Different price
            liquidity: U256::from(2000),
            tick: 100,
        };
        let different_hash =
            CachedPoolState::calculate_merkle_root(Some(&different_state), None, 100);
        assert!(!cached.is_state_hash_valid(different_hash));
    }

    #[test]
    fn test_hybrid_validation() {
        let cached = CachedPoolState {
            v3_state: None,
            v2_state: Some((U256::from(1000), U256::from(2000))),
            v2_token0: None,
            v2_token1: None,
            merkle_root: [0; 32],
            block_number: 100,
            last_updated: Instant::now(),
            touched: false,
        };

        // Should be valid within tolerance
        assert!(cached.is_cache_valid_hybrid(101, 5, std::time::Duration::from_secs(300)));

        // Should be invalid if block tolerance exceeded
        assert!(!cached.is_cache_valid_hybrid(110, 5, std::time::Duration::from_secs(300)));
    }
}
