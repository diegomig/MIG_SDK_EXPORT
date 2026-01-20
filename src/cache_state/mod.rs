//! Cache management modules for JIT state fetcher
//!
//! Provides Merkle tree-based cache invalidation and state cache management.

pub mod state_cache;

// Re-export for convenience
pub use state_cache::{CachedPoolState, StateCacheManager, CacheValidationResult};
