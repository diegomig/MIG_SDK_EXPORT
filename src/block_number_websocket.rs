//! # Block Number WebSocket Subscription
//!
//! Provides real-time block number updates via WebSocket subscription to `eth_subscribe("newHeads")`.
//! Falls back to polling if WebSocket connection fails.
//!
//! ## Features
//!
//! - **Real-time Updates**: Subscribes to `eth_subscribe("newHeads")` for instant block notifications
//! - **Automatic Reconnection**: Exponential backoff on connection failures
//! - **Polling Fallback**: Lazy polling (1s interval) if WebSocket disconnected >5s
//! - **Integration**: Updates `BlockNumberCache` via `update_from_external()`

use ethers::prelude::*;
use ethers::providers::{Provider, Ws};
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{Result, Context};
use tokio::time::sleep;
use futures_util::StreamExt;
use log::{info, warn, error, debug};

/// WebSocket-based block number updater with automatic reconnection
pub struct BlockNumberWebSocket {
    provider_url: String,
    cache: Option<Arc<crate::block_number_cache::BlockNumberCache>>,
    reconnect_delay: Duration,
    max_reconnect_delay: Duration,
    polling_fallback_threshold: Duration,
}

impl BlockNumberWebSocket {
    /// Create a new WebSocket block number updater
    ///
    /// # Arguments
    ///
    /// * `provider_url` - WebSocket URL (e.g., `ws://127.0.0.1:8545` or `wss://...`)
    /// * `cache` - Optional `BlockNumberCache` to update (can be set later via `set_cache()`)
    pub fn new(provider_url: String) -> Self {
        Self {
            provider_url,
            cache: None,
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(60),
            polling_fallback_threshold: Duration::from_secs(5),
        }
    }

    /// Set the block number cache to update
    pub fn set_cache(mut self, cache: Arc<crate::block_number_cache::BlockNumberCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Start the WebSocket subscription loop
    ///
    /// This method spawns a background task that:
    /// 1. Connects to WebSocket and subscribes to `newHeads`
    /// 2. Processes block notifications and updates cache
    /// 3. Automatically reconnects on failure with exponential backoff
    /// 4. Falls back to polling if disconnected >5s
    pub async fn start(self) -> Result<()> {
        let mut reconnect_delay = self.reconnect_delay;
        let provider_url = self.provider_url.clone();
        let cache = self.cache.clone();
        let polling_threshold = self.polling_fallback_threshold;
        let max_reconnect_delay = self.max_reconnect_delay;

        tokio::spawn(async move {
            loop {
                match Self::connect_and_subscribe(&provider_url, cache.clone(), polling_threshold).await {
                    Ok(()) => {
                        // Connection successful, reset delay
                        reconnect_delay = Duration::from_secs(1);
                        info!("✅ [BlockNumberWS] WebSocket connection successful, resetting reconnect delay");
                    }
                    Err(e) => {
                        error!("❌ [BlockNumberWS] WebSocket connection failed: {}. Reconnecting in {:?}...", e, reconnect_delay);
                    }
                }

                // Exponential backoff
                sleep(reconnect_delay).await;
                reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
            }
        });

        Ok(())
    }

    /// Connect to WebSocket and subscribe to newHeads
    async fn connect_and_subscribe(
        provider_url: &str,
        cache: Option<Arc<crate::block_number_cache::BlockNumberCache>>,
        polling_threshold: Duration,
    ) -> Result<()> {
        // Convert HTTP URL to WebSocket URL if needed
        let ws_url = if provider_url.starts_with("http://") {
            provider_url.replace("http://", "ws://")
        } else if provider_url.starts_with("https://") {
            provider_url.replace("https://", "wss://")
        } else if provider_url.starts_with("ws://") || provider_url.starts_with("wss://") {
            provider_url.to_string()
        } else {
            // Assume localhost HTTP, convert to WS
            format!("ws://{}", provider_url)
        };

        info!("🔌 [BlockNumberWS] Connecting to WebSocket: {}", ws_url);

        // Connect to WebSocket
        let provider = Provider::<Ws>::connect(&ws_url).await
            .context("Failed to connect to WebSocket provider")?;

        info!("✅ [BlockNumberWS] WebSocket connected, subscribing to newHeads...");

        // Subscribe to newHeads using subscribe_blocks
        let mut stream = provider.subscribe_blocks().await
            .context("Failed to subscribe to newHeads")?;

        info!("✅ [BlockNumberWS] Subscribed to newHeads");

        let mut last_block_update = Instant::now();
        let mut last_poll_time = Instant::now();
        let mut polling_active = false;
        let mut poll_interval = tokio::time::interval(Duration::from_secs(1));

        // Process block notifications
        loop {
            tokio::select! {
                // Receive new block notification
                block_opt = stream.next() => {
                    match block_opt {
                        Some(block) => {
                            if let Some(block_number) = block.number {
                                let block_u64 = block_number.as_u64();
                                
                                // Update cache if available
                                if let Some(ref cache_ref) = cache {
                                    cache_ref.update_from_external(block_u64);
                                }
                                
                                last_block_update = Instant::now();
                                polling_active = false; // Disable polling when WS is active
                                
                                debug!("📡 [BlockNumberWS] New block: {} (latency: {:?})", 
                                      block_u64, last_block_update.elapsed());
                            }
                        }
                        None => {
                            warn!("⚠️ [BlockNumberWS] WebSocket stream ended");
                            break;
                        }
                    }
                }
                
                // Polling fallback: check if WebSocket has been silent >5s
                _ = poll_interval.tick() => {
                    let time_since_last_block = last_block_update.elapsed();
                    
                    if time_since_last_block > polling_threshold && !polling_active {
                        // WebSocket silent >5s, activate polling fallback
                        polling_active = true;
                        warn!("⚠️ [BlockNumberWS] WebSocket silent for {:?}, activating polling fallback (1s interval)", 
                              time_since_last_block);
                    }
                    
                    if polling_active {
                        // Poll every 1 second when fallback is active
                        match provider.get_block_number().await {
                            Ok(block) => {
                                let block_u64 = block.as_u64();
                                if let Some(ref cache_ref) = cache {
                                    cache_ref.update_from_external(block_u64);
                                }
                                last_poll_time = Instant::now();
                                debug!("🔄 [BlockNumberWS] Polling fallback: block {}", block_u64);
                                
                                // If we got a block via polling, check if WS is back
                                if time_since_last_block < polling_threshold {
                                    polling_active = false;
                                    info!("✅ [BlockNumberWS] WebSocket appears to be back, disabling polling fallback");
                                }
                            }
                            Err(e) => {
                                warn!("⚠️ [BlockNumberWS] Polling fallback failed: {}", e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_websocket_url_conversion() {
        // Test HTTP to WS conversion
        let http_url = "http://127.0.0.1:8545";
        let ws = BlockNumberWebSocket::new(http_url.to_string());
        // URL conversion happens in connect_and_subscribe, not in constructor
        // This test just verifies the struct can be created
        assert_eq!(ws.provider_url, http_url);
    }
}

