// routegen-rs/src/coingecko_price_updater.rs

use std::sync::Arc;
use std::collections::HashMap;
use tokio::time::{interval, Duration, Instant};
use ethers::prelude::Address;
use log::{info, warn, error, debug};
use anyhow::Result;
use serde::Deserialize;
use crate::background_price_updater::{SharedPriceCache, PriceSource};

/// CoinGecko price updater - actualiza precios 10 veces/segundo desde CoinGecko API
/// Completamente asíncrono, no afecta el hot path
pub struct CoinGeckoPriceUpdater {
    cache: SharedPriceCache,
    token_map: HashMap<Address, String>, // Address -> CoinGecko ID
    update_interval: Duration,
    client: reqwest::Client,
}

// CoinGecko API devuelve directamente un HashMap<String, CoinGeckoTokenPrice>
// donde la key es el ID del token y el valor es { "usd": price }
type CoinGeckoPriceResponse = HashMap<String, CoinGeckoTokenPrice>;

#[derive(Debug, Deserialize)]
struct CoinGeckoTokenPrice {
    usd: f64,
}

impl CoinGeckoPriceUpdater {
    pub fn new(cache: SharedPriceCache) -> Self {
        // Mapeo de addresses de tokens a CoinGecko IDs
        let mut token_map = HashMap::new();
        
        // Tokens principales en Arbitrum
        token_map.insert(
            "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".parse().unwrap(), // WETH
            "weth".to_string(),
        );
        token_map.insert(
            "0x912ce59144191c1204e64559fe8253a0e49e6548".parse().unwrap(), // ARB
            "arbitrum".to_string(),
        );
        token_map.insert(
            "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f".parse().unwrap(), // WBTC
            "wrapped-bitcoin".to_string(),
        );
        token_map.insert(
            "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4".parse().unwrap(), // LINK
            "chainlink".to_string(),
        );
        token_map.insert(
            "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1".parse().unwrap(), // DAI
            "dai".to_string(),
        );
        token_map.insert(
            "0xba5DdD1f9d7F570dc94a51479a000E3BCE967196".parse().unwrap(), // AAVE
            "aave-token".to_string(),
        );
        token_map.insert(
            "0x5979D7b546E38E414F7E9822514be443A4800529".parse().unwrap(), // wstETH
            "wrapped-steth".to_string(),
        );
        token_map.insert(
            "0xEC70Dcb4A1EFa46b8F2D97C310C9c4790ba5ffA8".parse().unwrap(), // rETH
            "rocket-pool-eth".to_string(),
        );
        token_map.insert(
            "0x17fC002b466Eec40dae837fc4bE5C67993DDDc84".parse().unwrap(), // FRAX
            "frax".to_string(),
        );
        token_map.insert(
            "0x35751007a407cA6FDEffe3b56c4e27da25D5c8D5".parse().unwrap(), // weETH
            "wrapped-eeth".to_string(),
        );
        
        // Stablecoins (menos críticos pero útiles)
        token_map.insert(
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".parse().unwrap(), // USDC (native)
            "usd-coin".to_string(),
        );
        token_map.insert(
            // Correct USDC.e on Arbitrum One:
            "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8".parse().unwrap(), // USDC.e
            "usd-coin".to_string(), // Mismo ID que USDC
        );
        token_map.insert(
            "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9".parse().unwrap(), // USDT
            "tether".to_string(),
        );
        
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500)) // Timeout corto para no bloquear
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            cache,
            token_map,
            update_interval: Duration::from_millis(100), // 10 veces/segundo
            client,
        }
    }
    
    /// Inicia el updater en background
    pub async fn start(self: Arc<Self>) {
        info!("🚀 Starting CoinGecko price updater");
        info!("   Tokens: {}", self.token_map.len());
        info!("   Interval: {:?} (10 updates/sec)", self.update_interval);
        
        let mut ticker = interval(self.update_interval);
        let mut iteration = 0u64;
        let mut consecutive_failures = 0u32;
        
        loop {
            ticker.tick().await;
            iteration += 1;
            
            match self.update_prices().await {
                Ok(count) => {
                    if count > 0 {
                        debug!("✅ [CoinGecko #{}] Updated {} prices", iteration, count);
                        consecutive_failures = 0;
                    } else {
                        debug!("⚠️ [CoinGecko #{}] No prices updated", iteration);
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    if consecutive_failures % 10 == 0 {
                        // Solo log cada 10 fallos para no spamear
                        warn!("⚠️ [CoinGecko #{}] Update failed (consecutive failures: {}): {}", 
                              iteration, consecutive_failures, e);
                    }
                    
                    if consecutive_failures >= 50 {
                        error!("🚨 CoinGecko updater has {} consecutive failures, may be rate limited", 
                              consecutive_failures);
                        // Reset counter después de loggear para no spamear
                        consecutive_failures = 0;
                    }
                }
            }
        }
    }
    
    async fn update_prices(&self) -> Result<usize> {
        let start = Instant::now();
        
        // Construir lista de CoinGecko IDs únicos (algunos tokens comparten ID, ej: USDC y USDC.e)
        let mut coingecko_ids: Vec<String> = self.token_map.values()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        
        if coingecko_ids.is_empty() {
            return Ok(0);
        }
        
        // Construir URL de CoinGecko API
        // Usar simple/price endpoint que es más rápido
        let ids_param = coingecko_ids.join(",");
        let url = format!(
            "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
            ids_param
        );
        
        // Hacer request con timeout corto
        let response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(anyhow::anyhow!("HTTP request failed: {}", e));
            }
        };
        
        if !response.status().is_success() {
            if response.status() == 429 {
                return Err(anyhow::anyhow!("Rate limited (429)"));
            }
            return Err(anyhow::anyhow!("HTTP error: {}", response.status()));
        }
        
        let price_data: CoinGeckoPriceResponse = match response.json().await {
            Ok(d) => d,
            Err(e) => {
                return Err(anyhow::anyhow!("JSON parse failed: {}", e));
            }
        };
        
        // Mapear precios de CoinGecko IDs a addresses
        let mut prices = HashMap::new();
        let mut updated_count = 0;
        
        for (address, coingecko_id) in &self.token_map {
            if let Some(token_price) = price_data.get(coingecko_id) {
                let price = token_price.usd;
                
                // Validar precio razonable
                if price > 0.0 && price >= 0.0001 && price <= 1_000_000.0 {
                    prices.insert(*address, price);
                    updated_count += 1;
                } else {
                    warn!("⚠️ CoinGecko: Invalid price for {} ({}): ${:.2}", coingecko_id, address, price);
                }
            }
        }
        
        // Actualizar cache
        if !prices.is_empty() {
            self.cache.update_batch(prices, PriceSource::Chainlink); // Usar Chainlink como source para indicar precio externo confiable
        }
        
        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(50) {
            warn!("⚠️ CoinGecko update took {:?} (>50ms target)", elapsed);
        }
        
        Ok(updated_count)
    }
}

