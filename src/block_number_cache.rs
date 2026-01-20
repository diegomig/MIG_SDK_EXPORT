use ethers::prelude::{Middleware, Provider, Http};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use anyhow::Result;
use tracing::{debug, warn};
use crate::flight_recorder::FlightRecorder;
use crate::{record_rpc_call};
use crate::rpc_tracing_middleware::estimate_cu_cost;
use crate::metrics;

/// Global block number cache to minimize RPC calls
/// Updates at most once per second (configurable)
pub struct BlockNumberCache {
    current_block: Arc<AtomicU64>,
    last_update: Arc<Mutex<Instant>>,
    update_interval: Duration,
    provider: Arc<Provider<Http>>,
    flight_recorder: Option<Arc<FlightRecorder>>,
    endpoint: String,
}

impl BlockNumberCache {
    /// Create a new BlockNumberCache
    /// 
    /// # Arguments
    /// * `provider` - RPC provider to fetch block numbers
    /// * `update_interval` - Minimum time between updates (default: 1 second)
    pub fn new(provider: Arc<Provider<Http>>, update_interval: Duration) -> Self {
        Self {
            current_block: Arc::new(AtomicU64::new(0)),
            last_update: Arc::new(Mutex::new(Instant::now())),
            update_interval,
            provider,
            flight_recorder: None,
            endpoint: "unknown".to_string(),
        }
    }

    /// Set flight recorder for RPC call recording
    pub fn with_flight_recorder(mut self, recorder: Option<Arc<FlightRecorder>>, endpoint: String) -> Self {
        self.flight_recorder = recorder;
        self.endpoint = endpoint;
        self
    }

    /// Get current block number (cached, updates at most once per interval)
    /// 
    /// This method will:
    /// 1. Check if cache is fresh (updated within update_interval)
    /// 2. If fresh, return cached value immediately (NO RPC call)
    /// 3. If stale, fetch new block number and update cache (1 RPC call)
    pub async fn get_current_block(&self) -> Result<u64> {
        let now = Instant::now();
        let mut last_update = self.last_update.lock().await;
        
        // Check if cache is fresh
        if now.duration_since(*last_update) < self.update_interval {
            let cached = self.current_block.load(Ordering::Relaxed);
            if cached > 0 {
                debug!("BlockNumberCache: Using cached block {}", cached);
                return Ok(cached);
            }
        }
        
        // Cache is stale or empty, fetch new block number
        debug!("BlockNumberCache: Fetching new block number (cache stale or empty)");
        let start = std::time::Instant::now();
        let method = "eth_blockNumber";
        match self.provider.get_block_number().await {
            Ok(block) => {
                let block_u64 = block.as_u64();
                let duration = start.elapsed();
                let success = true;
                
                // Registrar métricas
                let component = "block_number_cache";
                let cu_cost = estimate_cu_cost(method, 0);
                metrics::increment_rpc_call(component);
                metrics::increment_rpc_call_by_method(component, method);
                metrics::record_rpc_cu_cost(component, method, cu_cost);
                metrics::record_rpc_payload_size(component, method, 0);
                metrics::record_rpc_call_latency(component, method, duration);
                
                // ✅ FLIGHT RECORDER: Registrar evento RPC
                if let Some(ref recorder) = self.flight_recorder {
                    record_rpc_call!(recorder, &self.endpoint, method, start, success);
                }
                
                self.current_block.store(block_u64, Ordering::Relaxed);
                *last_update = now;
                debug!("BlockNumberCache: Updated to block {}", block_u64);
                Ok(block_u64)
            }
            Err(e) => {
                // Registrar error en Flight Recorder
                let duration = start.elapsed();
                let success = false;
                if let Some(ref recorder) = self.flight_recorder {
                    record_rpc_call!(recorder, &self.endpoint, method, start, success);
                }
                
                // On error, return cached value if available, otherwise return error
                let cached = self.current_block.load(Ordering::Relaxed);
                if cached > 0 {
                    warn!("BlockNumberCache: Failed to fetch block number, using cached value {}: {}", cached, e);
                    Ok(cached)
                } else {
                    Err(anyhow::anyhow!("Failed to fetch block number and no cache available: {}", e))
                }
            }
        }
    }

    /// Force update block number (bypasses cache freshness check)
    /// Use this when you know the block number must be fresh (e.g., after a new block event)
    pub async fn force_update(&self) -> Result<u64> {
        debug!("BlockNumberCache: Force updating block number");
        let start = std::time::Instant::now();
        let method = "eth_blockNumber";
        match self.provider.get_block_number().await {
            Ok(block) => {
                let block_u64 = block.as_u64();
                let duration = start.elapsed();
                let success = true;
                
                // Registrar métricas
                let component = "block_number_cache";
                let cu_cost = estimate_cu_cost(method, 0);
                metrics::increment_rpc_call(component);
                metrics::increment_rpc_call_by_method(component, method);
                metrics::record_rpc_cu_cost(component, method, cu_cost);
                metrics::record_rpc_payload_size(component, method, 0);
                metrics::record_rpc_call_latency(component, method, duration);
                
                // ✅ FLIGHT RECORDER: Registrar evento RPC
                if let Some(ref recorder) = self.flight_recorder {
                    record_rpc_call!(recorder, &self.endpoint, method, start, success);
                }
                
                self.current_block.store(block_u64, Ordering::Relaxed);
                let mut last_update = self.last_update.lock().await;
                *last_update = Instant::now();
                debug!("BlockNumberCache: Force updated to block {}", block_u64);
                Ok(block_u64)
            }
            Err(e) => {
                // Registrar error en Flight Recorder
                let duration = start.elapsed();
                let success = false;
                if let Some(ref recorder) = self.flight_recorder {
                    record_rpc_call!(recorder, &self.endpoint, method, start, success);
                }
                
                let cached = self.current_block.load(Ordering::Relaxed);
                if cached > 0 {
                    warn!("BlockNumberCache: Force update failed, using cached value {}: {}", cached, e);
                    Ok(cached)
                } else {
                    Err(anyhow::anyhow!("Failed to force update block number and no cache available: {}", e))
                }
            }
        }
    }

    /// Get cached block number without triggering update
    /// Returns 0 if cache is empty
    pub fn get_cached(&self) -> u64 {
        self.current_block.load(Ordering::Relaxed)
    }

    /// Update block number from external source (e.g., WebSocket block event)
    /// This is more efficient than force_update() as it doesn't make an RPC call
    pub fn update_from_external(&self, block_number: u64) {
        if block_number > 0 {
            self.current_block.store(block_number, Ordering::Relaxed);
            // Note: We don't update last_update here to allow get_current_block()
            // to still work correctly (it will see the cache is fresh)
            debug!("BlockNumberCache: Updated from external source to block {}", block_number);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_cache_freshness() {
        // This test would require a mock provider
        // For now, just verify the structure compiles
        let update_interval = Duration::from_secs(1);
        // Would need mock provider for actual test
    }
}

