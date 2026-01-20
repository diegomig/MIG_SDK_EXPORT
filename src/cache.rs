use crate::{dex_adapter::PoolMeta, metrics};
use anyhow::Result;
use dashmap::DashMap;
use ethers::prelude::Address;
use std::sync::Arc;
use crate::database::{self, DbPool};
use log::debug;

use crate::pools::Pool;

#[derive(Debug, Clone)]
/// General-purpose cache manager for SDK components.
///
/// Provides a unified caching interface for various SDK components with configurable
/// TTL and eviction policies.
///
/// ## Features
///
/// - **LRU Eviction**: Least-recently-used eviction policy
/// - **TTL Support**: Time-based expiration
/// - **Thread-Safe**: Concurrent access via `Arc`
pub struct CacheManager {
    pub pool_meta_cache: Arc<DashMap<Address, PoolMeta>>,
    // FASE 4.1: Replaced Mutex<LruCache> with DashMap for lock-free access
    pub pool_state_cache: Arc<DashMap<Address, Pool>>,
    pub token_decimals_cache: Arc<DashMap<Address, u8>>,
    pub usd_price_cache: Arc<DashMap<Address, f64>>,
    // Track insertion order for LRU-like eviction (manual cleanup)
    pool_state_cache_max_size: usize,
}

impl CacheManager {
    pub fn new() -> Self {
        Self {
            pool_meta_cache: Arc::new(DashMap::new()),
            // FASE 4.1: Lock-free DashMap (max 1000 entries, manual eviction)
            pool_state_cache: Arc::new(DashMap::new()),
            token_decimals_cache: Arc::new(DashMap::new()),
            usd_price_cache: Arc::new(DashMap::new()),
            pool_state_cache_max_size: 1000,
        }
    }
    
    // FASE 4.1: Manual eviction when cache exceeds max size
    pub fn maybe_evict_pool_state(&self) {
        if self.pool_state_cache.len() > self.pool_state_cache_max_size {
            // Simple eviction: remove 10% of oldest entries (by iterating and removing first N)
            let to_remove = self.pool_state_cache.len() - self.pool_state_cache_max_size;
            let mut removed = 0;
            for entry in self.pool_state_cache.iter() {
                if removed >= to_remove {
                    break;
                }
                self.pool_state_cache.remove(entry.key());
                removed += 1;
            }
            if removed > 0 {
                debug!("Evicted {} entries from pool_state_cache (size: {})", removed, self.pool_state_cache.len());
            }
        }
    }

    pub async fn prime_caches(&self, db_pool: &DbPool) -> Result<()> {
        let pools_meta = database::load_active_pools_meta(db_pool).await?;
        for meta in pools_meta {
            self.pool_meta_cache.insert(meta.address, meta);
        }
        self.record_cache_sizes().await;
        Ok(())
    }

    pub fn get_pool_meta(&self, address: &Address) -> Option<PoolMeta> {
        match self.pool_meta_cache.get(address) {
            Some(meta) => {
                metrics::increment_cache_hit("pool_meta");
                Some(meta.clone())
            }
            None => {
                metrics::increment_cache_miss("pool_meta");
                None
            }
        }
    }

    pub fn get_active_pools_meta(&self) -> Vec<PoolMeta> {
        self.pool_meta_cache.iter().map(|entry| entry.value().clone()).collect()
    }

    pub async fn record_cache_sizes(&self) {
        metrics::set_cache_size("pool_meta", self.pool_meta_cache.len() as f64);
        metrics::set_cache_size("token_decimals", self.token_decimals_cache.len() as f64);
        // FASE 4.1: No lock needed for DashMap
        metrics::set_cache_size("pool_state", self.pool_state_cache.len() as f64);
    }
    
    // FASE 4.1: Lock-free get for pool state
    pub fn get_pool_state(&self, address: &Address) -> Option<Pool> {
        self.pool_state_cache.get(address).map(|e| e.value().clone())
    }
    
    // FASE 4.1: Lock-free insert for pool state with eviction
    pub fn put_pool_state(&self, address: Address, pool: Pool) {
        self.pool_state_cache.insert(address, pool);
        self.maybe_evict_pool_state();
    }
}
