// Redis Manager - Cache & Coordination Layer for Topology SDK
// Implements pool state cache and route cache

use anyhow::Result;
#[cfg(feature = "redis")]
use anyhow::Context;
#[cfg(feature = "redis")]
use redis::aio::ConnectionManager;
#[cfg(feature = "redis")]
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use log::{info, debug};


/// Cached pool state for Redis storage.
///
/// Stores pool state with metadata for cache invalidation and freshness checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPoolState {
    pub address: String,
    pub reserve0: Option<String>,
    pub reserve1: Option<String>,
    pub sqrt_price_x96: Option<String>,
    pub tick: Option<i32>,
    pub liquidity: Option<String>,
    pub block_number: u64,
    pub timestamp: i64,
}

/// Configuration for Redis connection and caching behavior.
#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub pool_state_ttl: u64,      // 10 seconds
    pub route_cache_ttl: u64,      // 60 seconds
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
            pool_state_ttl: 10,
            route_cache_ttl: 60,
        }
    }
}

/// Main Redis Manager for bot coordination
#[cfg(feature = "redis")]
pub struct RedisManager {
    conn: ConnectionManager,
    config: RedisConfig,
}

#[cfg(not(feature = "redis"))]
pub struct RedisManager {
    config: RedisConfig,
    // NOTE: conn field removed when redis feature is disabled
    _phantom: std::marker::PhantomData<()>,
}

impl RedisManager {
    /// Create new Redis Manager
    #[cfg(feature = "redis")]
    pub async fn new(config: RedisConfig) -> Result<Self> {
        let client = Client::open(config.url.as_str())
            .context("Failed to create Redis client")?;
        
        let conn = ConnectionManager::new(client.clone())
            .await
            .context("Failed to connect to Redis")?;
        
        info!("✅ Redis Manager connected to {}", config.url);
        
        Ok(Self { conn, config })
    }
    
    #[cfg(not(feature = "redis"))]
    pub async fn new(config: RedisConfig) -> Result<Self> {
        Err(anyhow::anyhow!("Redis feature not enabled. Enable with 'redis' feature flag."))
    }

    /// Create with default localhost config
    pub async fn new_default() -> Result<Self> {
        Self::new(RedisConfig::default()).await
    }

    // ==================== POOL STATE CACHE ====================
    
    /// Cache pool state
    /// FASE 2.1: Uses bincode for faster serialization than JSON
    #[cfg(feature = "redis")]
    pub async fn cache_pool_state(&mut self, state: &CachedPoolState) -> Result<()> {
        let key = format!("pool:state:{}", state.address);
        // FASE 2.1: Use bincode instead of JSON for faster serialization
        let bytes = bincode::serialize(state)
            .context("Failed to serialize pool state with bincode")?;
        
        self.conn.set_ex::<_, _, ()>(&key, &bytes, self.config.pool_state_ttl)
            .await
            .context("Failed to cache pool state")?;
        
        debug!("💾 Cached state for pool {}", state.address);
        
        Ok(())
    }

    /// Get cached pool state
    /// FASE 2.1: Uses bincode for faster deserialization than JSON
    #[cfg(feature = "redis")]
    pub async fn get_pool_state(&mut self, address: &str) -> Result<Option<CachedPoolState>> {
        let start = std::time::Instant::now();
        let key = format!("pool:state:{}", address);
        
        // FASE 2.1: Changed from String to Vec<u8> for binary data
        let bytes: Option<Vec<u8>> = self.conn.get(&key).await
            .context("Failed to get pool state from cache")?;
        
        // FASE 3: Record metrics
        crate::metrics::increment_redis_operation("get_pool_state");
        crate::metrics::record_redis_operation_duration("get_pool_state", start.elapsed());
        
        if let Some(bytes) = bytes {
            // FASE 2.1: Use bincode instead of JSON for faster deserialization
            let state: CachedPoolState = bincode::deserialize(bytes.as_slice())
                .context("Failed to deserialize pool state with bincode")?;
            
            debug!("📖 Cache hit for pool {}", address);
            crate::metrics::increment_redis_cache_hit();
            Ok(Some(state))
        } else {
            debug!("❌ Cache miss for pool {}", address);
            crate::metrics::increment_redis_cache_miss();
            Ok(None)
        }
    }

    #[cfg(not(feature = "redis"))]
    pub async fn get_pool_state(&mut self, _address: &str) -> Result<Option<CachedPoolState>> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }

    /// Batch cache pool states
    #[cfg(feature = "redis")]
    pub async fn batch_cache_pool_states(&mut self, states: &[CachedPoolState]) -> Result<()> {
        for state in states {
            self.cache_pool_state(state).await?;
        }
        info!("💾 Cached {} pool states", states.len());
        Ok(())
    }

    // ==================== ROUTE CACHE ====================
    
    #[cfg(not(feature = "redis"))]
    pub async fn batch_cache_pool_states(&mut self, _states: &[CachedPoolState]) -> Result<()> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }

    /// Cache route for a token pair
    /// FASE 2.1: Note: This method accepts String for backward compatibility
    /// For new code, consider using bincode serialization
    #[cfg(feature = "redis")]
    pub async fn cache_route(&mut self, token0: &str, token1: &str, route_json: &str) -> Result<()> {
        let key = format!("route:{}:{}", token0, token1);
        
        // FASE 2.1: Keep as String for backward compatibility (route_cache.rs still uses JSON)
        self.conn.set_ex::<_, _, ()>(&key, route_json, self.config.route_cache_ttl)
            .await
            .context("Failed to cache route")?;
        
        debug!("🗺️  Cached route for pair {}/{}", token0, token1);
        
        Ok(())
    }

    /// Get cached route
    /// FASE 2.1: Note: Returns String for backward compatibility
    #[cfg(feature = "redis")]
    pub async fn get_cached_route(&mut self, token0: &str, token1: &str) -> Result<Option<String>> {
        let start = std::time::Instant::now();
        let key = format!("route:{}:{}", token0, token1);
        
        // FASE 2.1: Keep as String for backward compatibility
        let route: Option<String> = self.conn.get(&key).await
            .context("Failed to get cached route")?;
        
        // FASE 3: Record metrics
        crate::metrics::increment_redis_operation("get_cached_route");
        crate::metrics::record_redis_operation_duration("get_cached_route", start.elapsed());
        
        if route.is_some() {
            debug!("🗺️  Cache hit for route {}/{}", token0, token1);
            crate::metrics::increment_redis_cache_hit();
        } else {
            debug!("❌ Cache miss for route {}/{}", token0, token1);
            crate::metrics::increment_redis_cache_miss();
        }
        
        Ok(route)
    }

    // ==================== EXECUTION TRACKER ====================
    
    #[cfg(not(feature = "redis"))]
    pub async fn get_cached_route(&mut self, _token0: &str, _token1: &str) -> Result<Option<String>> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }


    /// Replace dynamic allowed pairs set for MVP auto filter
    #[cfg(feature = "redis")]
    pub async fn set_mvp_allowed_pairs(&mut self, pairs: &[(String, String)], ttl_secs: u64) -> Result<()> {
        let key = "mvp:allowed_pairs";
        // Replace the set atomically: DEL then SADD all members
        let _: () = self.conn.del(key).await.unwrap_or(());
        if !pairs.is_empty() {
            // Flatten pairs into "token0:token1" strings to store in a Redis set
            let members: Vec<String> = pairs.iter().map(|(a, b)| format!("{}:{}", a, b)).collect();
            let _: () = self.conn.sadd(key, members).await
                .context("Failed to SADD mvp:allowed_pairs")?;
            // Set TTL
            let _: () = self.conn.expire(key, ttl_secs as i64).await
                .context("Failed to set TTL for mvp:allowed_pairs")?;
        }
        Ok(())
    }

    /// Read current allowed pairs from Redis
    #[cfg(feature = "redis")]
    pub async fn get_mvp_allowed_pairs(&mut self) -> Result<Vec<(String, String)>> {
        let key = "mvp:allowed_pairs";
        let members: Vec<String> = self.conn.smembers(key).await
            .context("Failed to SMEMBERS mvp:allowed_pairs")?;
        let mut out = Vec::with_capacity(members.len());
        for m in members {
            if let Some((a, b)) = m.split_once(':') {
                out.push((a.to_string(), b.to_string()));
            }
        }
        Ok(out)
    }

    // ==================== METRICS & MONITORING ====================
    
    #[cfg(not(feature = "redis"))]
    pub async fn get_mvp_allowed_pairs(&mut self) -> Result<Vec<(String, String)>> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }


    // ==================== HEALTH CHECK ====================
    
    /// Test Redis connection
    #[cfg(feature = "redis")]
    pub async fn health_check(&mut self) -> Result<()> {
        let pong: String = redis::cmd("PING")
            .query_async(&mut self.conn)
            .await
            .context("Redis health check failed")?;
        
        if pong == "PONG" {
            Ok(())
        } else {
            anyhow::bail!("Unexpected Redis response: {}", pong)
        }
    }
    
    #[cfg(not(feature = "redis"))]
    pub async fn health_check(&mut self) -> Result<()> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }

    /// Get Redis info
    #[cfg(feature = "redis")]
    pub async fn get_info(&mut self) -> Result<String> {
        let info: String = redis::cmd("INFO")
            .arg("stats")
            .query_async(&mut self.conn)
            .await
            .context("Failed to get Redis info")?;
        
        Ok(info)
    }
    
    #[cfg(not(feature = "redis"))]
    pub async fn get_info(&mut self) -> Result<String> {
        Err(anyhow::anyhow!("Redis feature not enabled"))
    }

    // ==================== CLEANUP ====================
    
    #[cfg(not(feature = "redis"))]
    pub async fn flush_opportunities(&mut self) -> Result<()> {
        Ok(()) // Redis not available - no-op
    }

    /// Clear pool state cache
    #[cfg(feature = "redis")]
    pub async fn clear_pool_cache(&mut self) -> Result<()> {
        let keys: Vec<String> = self.conn.keys("pool:state:*").await
            .context("Failed to get pool cache keys")?;
        
        if !keys.is_empty() {
            self.conn.del::<_, ()>(keys).await
                .context("Failed to clear pool cache")?;
            
            info!("🗑️  Cleared pool state cache");
        }
        
        Ok(())
    }
    
    #[cfg(not(feature = "redis"))]
    pub async fn clear_pool_cache(&mut self) -> Result<()> {
        Ok(()) // Redis not available - no-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires Redis running
    async fn test_redis_connection() {
        let manager = RedisManager::new_default().await;
        assert!(manager.is_ok());
    }


    #[tokio::test]
    #[cfg(feature = "redis")]
    #[ignore] // Requires Redis running
    async fn test_pool_state_cache() {
        let mut manager = RedisManager::new_default().await.unwrap();
        
        let state = CachedPoolState {
            address: "0x123".to_string(),
            reserve0: Some("1000000".to_string()),
            reserve1: Some("2000000".to_string()),
            sqrt_price_x96: None,
            tick: None,
            liquidity: None,
            block_number: 12345,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        manager.cache_pool_state(&state).await.unwrap();
        
        let cached = manager.get_pool_state("0x123").await.unwrap();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().address, "0x123");
    }
}

