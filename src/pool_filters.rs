// Professional Pool Filtering System
// Filters pools based on liquidity, price alignment, freshness, and structural quality
// This is what separates professional bots from amateur ones

use crate::hot_pool_manager::{V2PoolSnapshot, V3PoolSnapshot};
use crate::metrics;
use crate::price_feeds::PriceFeed;
use crate::settings::Settings;
use ethers::prelude::Middleware;
use ethers::types::{Address, U256};
use log::{debug, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration for professional pool filtering
#[derive(Debug, Clone)]
pub struct PoolFilterConfig {
    // 1. Effective liquidity filter
    pub min_effective_liquidity_eth: f64, // Minimum liquidity in ETH equivalent
    
    // 2. Global price deviation filter
    pub max_price_deviation_bps: u32, // Maximum deviation from global price (in basis points)
    
    // 3. Stale data filter
    pub max_stale_blocks: u64, // Maximum blocks since last update
    
    // 4. Volume filter
    pub min_volume_24h_usd: f64, // Minimum 24h volume in USD
    
    // 5. Fee tier filter
    pub allowed_fee_tiers: Vec<u32>, // Allowed fee tiers (e.g., [5, 30, 100] for 0.05%, 0.3%, 1%)
    
    // 6. DEX whitelist
    pub allowed_dexs: Vec<String>, // Allowed DEX names
    
    // 7. Pool size vs trade size filter
    pub min_reserve_multiplier: f64, // reserve >= amount_in * multiplier (e.g., 5.0 = 5x)
}

impl Default for PoolFilterConfig {
    fn default() -> Self {
        Self {
            // Conservative defaults for professional use
            min_effective_liquidity_eth: 3.0, // 3 ETH minimum
            max_price_deviation_bps: 30,      // 0.3% max deviation
            max_stale_blocks: 3,              // 3 blocks max age
            min_volume_24h_usd: 50000.0,      // $50K minimum volume
            allowed_fee_tiers: vec![5, 30, 100], // 0.05%, 0.3%, 1%
            allowed_dexs: vec![
                "Uniswap V3".to_string(),
                "Sushiswap".to_string(),
                "Camelot".to_string(),
                "Kyber Elastic".to_string(),
                "Ramses V2".to_string(),
                "Ramses V3".to_string(),
                "Trader Joe".to_string(),
                "Balancer".to_string(),
            ],
            min_reserve_multiplier: 5.0, // Reserve must be 5x trade size
        }
    }
}

impl PoolFilterConfig {
    /// Create PoolFilterConfig from Settings
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            min_effective_liquidity_eth: settings.pool_filters.min_effective_liquidity_eth,
            max_price_deviation_bps: settings.pool_filters.max_price_deviation_bps,
            max_stale_blocks: settings.pool_filters.max_stale_blocks,
            min_volume_24h_usd: settings.pool_filters.min_volume_24h_usd,
            allowed_fee_tiers: settings.pool_filters.allowed_fee_tiers.clone(),
            allowed_dexs: settings.pool_filters.allowed_dexs.clone(),
            min_reserve_multiplier: settings.pool_filters.min_reserve_multiplier,
        }
    }
}

/// Result of pool filtering
#[derive(Debug, Clone)]
pub struct PoolFilterResult {
    pub is_valid: bool,
    pub rejection_reason: Option<String>,
    pub effective_liquidity_eth: Option<f64>,
    pub price_deviation_bps: Option<f64>,
    pub blocks_since_update: Option<u64>,
}

/// Professional pool filter
pub struct PoolFilter<M: Middleware> {
    config: PoolFilterConfig,
    price_feed: Arc<PriceFeed<M>>,
    current_block: u64,
    global_prices: HashMap<Address, f64>, // Cache of global reference prices
    weth_price_cache: Option<(f64, Instant)>, // ✅ Cache de precio WETH con timestamp (actualizado desde CoinGecko 1-2 veces/día)
}

impl<M: Middleware> PoolFilter<M> {
    pub fn new(
        config: PoolFilterConfig,
        price_feed: Arc<PriceFeed<M>>,
        current_block: u64,
    ) -> Self {
        Self {
            config,
            price_feed,
            current_block,
            global_prices: HashMap::new(),
            weth_price_cache: None, // Se inicializa cuando se carga WETH desde PriceFeed
        }
    }

    pub fn update_block(&mut self, block: u64) {
        self.current_block = block;
    }

    /// Update global reference prices (should be called periodically)
    /// ✅ MEJORADO: Siempre incluye WETH para conversión precisa a ETH
    pub async fn update_global_prices(&mut self, tokens: &[Address])
    where
        M: Middleware + 'static,
    {
        use std::str::FromStr;
        
        // Cargar precios de tokens solicitados
        for token in tokens {
            if let Ok(price) = self.price_feed.get_usd_price(*token).await {
                self.global_prices.insert(*token, price);
            }
        }
        
        // ✅ MEJORADO: Siempre asegurar que WETH está en cache para estimate_eth_value
        // WETH address on Arbitrum: 0x82af49447d8a07e3bd95bd0d56f35241523fbab1
        let weth_address = Address::from_str("0x82af49447d8a07e3bd95bd0d56f35241523fbab1")
            .unwrap_or_else(|_| Address::zero());
        
        // ✅ MEJORADO: Cargar WETH desde PriceFeed (usa CoinGecko/external APIs con cache)
        // Este precio se actualiza periódicamente (1-2 veces/día) y se cachea con timestamp
        if let Ok(weth_price) = self.price_feed.get_usd_price(weth_address).await {
            self.global_prices.insert(weth_address, weth_price);
            // ✅ Cachear precio WETH con timestamp para uso como fallback (< 24h)
            self.weth_price_cache = Some((weth_price, Instant::now()));
        } else {
            // Si falla obtener precio fresco, usar cache si tiene < 24 horas de antigüedad
            if let Some((cached_price, cached_time)) = &self.weth_price_cache {
                let age = cached_time.elapsed();
                if age < Duration::from_secs(24 * 60 * 60) {
                    // Cache válido (< 24 horas), usar precio cacheado
                    self.global_prices.insert(weth_address, *cached_price);
                } else {
                    warn!("⚠️ WETH price cache expired (>24h), price not available");
                }
            }
        }
    }

    /// Set global price directly (for cached prices, price_1e8 format)
    pub fn set_global_price(&mut self, token: Address, price_1e8: u128) {
        // Convert from 1e8 format to USD price
        let price_usd = price_1e8 as f64 / 1e8;
        self.global_prices.insert(token, price_usd);
    }

    /// Update global reference prices from reference pools (UniswapV3 0.05%, Camelot V3, etc.)
    /// This provides more accurate prices than oracle feeds for DEX-to-DEX price comparisons
    /// Note: This method is not currently used but available for future enhancement
    #[allow(dead_code)]
    pub async fn update_global_prices_from_reference_pools<M2>(
        &mut self,
        _reference_pools: &[(Address, Address, Address)], // (pool_address, token0, token1)
        _provider: Arc<M2>,
    ) where
        M2: Middleware + 'static,
    {
        // Implementation deferred - can be added later if needed
        // For now, we rely on update_global_prices which uses PriceFeed
    }

    /// Filter V2 pool
    pub async fn filter_v2_pool(
        &self,
        snapshot: &V2PoolSnapshot,
        amount_in: U256,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        decimals_out: u8,
    ) -> PoolFilterResult {
        // 1. Check DEX whitelist
        // Note: V2PoolSnapshot doesn't have dex field, so we skip this for now
        // This should be checked at a higher level

        // 2. Check effective liquidity
        let effective_liquidity_result = self.check_effective_liquidity_v2(
            snapshot,
            token_in,
            token_out,
            decimals_in,
            decimals_out,
        );

        if !effective_liquidity_result.is_valid {
            return effective_liquidity_result;
        }

        // 3. Check price deviation from global
        let price_deviation_result = self.check_price_deviation_v2(
            snapshot,
            token_in,
            token_out,
            decimals_in,
            decimals_out,
        ).await;

        if !price_deviation_result.is_valid {
            return price_deviation_result;
        }

        // 4. Check stale data
        let stale_result = self.check_stale_data(snapshot.last_updated);
        if !stale_result.is_valid {
            return stale_result;
        }

        // 5. Check reserve vs trade size
        let reserve_check = self.check_reserve_size_v2(
            snapshot,
            amount_in,
            token_in,
            decimals_in,
        );

        if !reserve_check.is_valid {
            return reserve_check;
        }

        // All checks passed
        metrics::increment_pool_filter_passed();
        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: effective_liquidity_result.effective_liquidity_eth,
            price_deviation_bps: price_deviation_result.price_deviation_bps,
            blocks_since_update: stale_result.blocks_since_update,
        }
    }

    /// Filter V3 pool
    pub async fn filter_v3_pool(
        &self,
        snapshot: &V3PoolSnapshot,
        amount_in: U256,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        decimals_out: u8,
    ) -> PoolFilterResult {
        // 1. Check DEX whitelist
        if !self.config.allowed_dexs.contains(&snapshot.dex.to_string()) {
            metrics::increment_pool_filter_rejected("dex_not_whitelist");
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!("DEX '{}' not in whitelist", snapshot.dex)),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        // 2. Check fee tier
        if !self.config.allowed_fee_tiers.contains(&snapshot.fee) {
            metrics::increment_pool_filter_rejected("fee_tier_not_allowed");
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!("Fee tier {} not allowed", snapshot.fee)),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        // 3. Check effective liquidity
        let effective_liquidity_result = self.check_effective_liquidity_v3(
            snapshot,
            token_in,
            token_out,
            decimals_in,
            decimals_out,
        );

        if !effective_liquidity_result.is_valid {
            return effective_liquidity_result;
        }

        // 4. Check price deviation from global
        let price_deviation_result = self.check_price_deviation_v3(
            snapshot,
            token_in,
            token_out,
            decimals_in,
            decimals_out,
        ).await;

        if !price_deviation_result.is_valid {
            return price_deviation_result;
        }

        // 5. Check stale data
        let stale_result = self.check_stale_data(snapshot.last_updated);
        if !stale_result.is_valid {
            return stale_result;
        }

        // 6. Check liquidity vs trade size (V3 uses liquidity, not reserves)
        let liquidity_check = self.check_liquidity_size_v3(
            snapshot,
            amount_in,
            token_in,
            decimals_in,
        );

        if !liquidity_check.is_valid {
            return liquidity_check;
        }

        // All checks passed
        metrics::increment_pool_filter_passed();
        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: effective_liquidity_result.effective_liquidity_eth,
            price_deviation_bps: price_deviation_result.price_deviation_bps,
            blocks_since_update: stale_result.blocks_since_update,
        }
    }

    // ========== Individual Filter Checks ==========

    /// Check 1: Effective liquidity (min(reserve_in, reserve_out) in ETH)
    fn check_effective_liquidity_v2(
        &self,
        snapshot: &V2PoolSnapshot,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        _decimals_out: u8,
    ) -> PoolFilterResult {
        // Determine which reserve corresponds to token_in
        let reserve_in = if token_in == snapshot.token0 {
            snapshot.reserve0
        } else {
            snapshot.reserve1
        };

        let reserve_out = if token_out == snapshot.token0 {
            snapshot.reserve0
        } else {
            snapshot.reserve1
        };

        // Calculate effective liquidity (minimum of both sides)
        let effective_liquidity = if reserve_in < reserve_out {
            reserve_in
        } else {
            reserve_out
        };

        // Convert to ETH equivalent (we need token prices)
        // For now, we'll use a simplified check: ensure reserves are non-zero and reasonable
        // TODO: Convert to ETH using price feed
        let effective_liquidity_eth = self.estimate_eth_value(
            effective_liquidity,
            token_in,
            decimals_in,
        );

        if effective_liquidity_eth < self.config.min_effective_liquidity_eth {
            metrics::increment_pool_filter_effective_liquidity_too_low();
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!(
                    "Effective liquidity too low: {:.2} ETH < {:.2} ETH",
                    effective_liquidity_eth, self.config.min_effective_liquidity_eth
                )),
                effective_liquidity_eth: Some(effective_liquidity_eth),
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: Some(effective_liquidity_eth),
            price_deviation_bps: None,
            blocks_since_update: None,
        }
    }

    fn check_effective_liquidity_v3(
        &self,
        snapshot: &V3PoolSnapshot,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        _decimals_out: u8,
    ) -> PoolFilterResult {
        // For V3, we use token balances as a proxy for effective liquidity
        let balance_in = if token_in == snapshot.token0 {
            snapshot.token0_balance
        } else {
            snapshot.token1_balance
        };

        let balance_out = if token_out == snapshot.token0 {
            snapshot.token0_balance
        } else {
            snapshot.token1_balance
        };

        let effective_balance = if balance_in < balance_out {
            balance_in
        } else {
            balance_out
        };

        let effective_liquidity_eth = self.estimate_eth_value(
            effective_balance,
            token_in,
            decimals_in,
        );

        if effective_liquidity_eth < self.config.min_effective_liquidity_eth {
            metrics::increment_pool_filter_effective_liquidity_too_low();
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!(
                    "Effective liquidity too low: {:.2} ETH < {:.2} ETH",
                    effective_liquidity_eth, self.config.min_effective_liquidity_eth
                )),
                effective_liquidity_eth: Some(effective_liquidity_eth),
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: Some(effective_liquidity_eth),
            price_deviation_bps: None,
            blocks_since_update: None,
        }
    }

    /// Check 2: Price deviation from global reference price
    async fn check_price_deviation_v2(
        &self,
        snapshot: &V2PoolSnapshot,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        decimals_out: u8,
    ) -> PoolFilterResult {
        // Calculate pool price: reserve_out / reserve_in (adjusted for decimals)
        let reserve_in = if token_in == snapshot.token0 {
            snapshot.reserve0
        } else {
            snapshot.reserve1
        };

        let reserve_out = if token_out == snapshot.token0 {
            snapshot.reserve0
        } else {
            snapshot.reserve1
        };

        if reserve_in.is_zero() || reserve_out.is_zero() {
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some("Zero reserves".to_string()),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        // Adjust for decimals
        let reserve_in_f64 = reserve_in.as_u128() as f64 / 10f64.powi(decimals_in as i32);
        let reserve_out_f64 = reserve_out.as_u128() as f64 / 10f64.powi(decimals_out as i32);

        let pool_price = reserve_out_f64 / reserve_in_f64; // token_out per token_in

        // Get global reference prices
        let global_price_in = self.global_prices.get(&token_in).copied();
        let global_price_out = self.global_prices.get(&token_out).copied();

        if let (Some(price_in), Some(price_out)) = (global_price_in, global_price_out) {
            // Calculate global price: price_out / price_in
            let global_price = price_out / price_in;

            // Calculate deviation
            let deviation = if global_price > 0.0 {
                ((pool_price - global_price) / global_price).abs()
            } else {
                1.0 // 100% deviation if global price is zero
            };

            let deviation_bps = (deviation * 10000.0) as u32;

            if deviation_bps > self.config.max_price_deviation_bps {
                metrics::increment_pool_filter_price_deviation_too_high();
                return PoolFilterResult {
                    is_valid: false,
                    rejection_reason: Some(format!(
                        "Price deviation too high: {} bps > {} bps",
                        deviation_bps, self.config.max_price_deviation_bps
                    )),
                    effective_liquidity_eth: None,
                    price_deviation_bps: Some(deviation_bps as f64),
                    blocks_since_update: None,
                };
            }

            PoolFilterResult {
                is_valid: true,
                rejection_reason: None,
                effective_liquidity_eth: None,
                price_deviation_bps: Some(deviation_bps as f64),
                blocks_since_update: None,
            }
        } else {
            // No global price available, skip this check
            debug!("No global price available for price deviation check");
            PoolFilterResult {
                is_valid: true,
                rejection_reason: None,
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            }
        }
    }

    async fn check_price_deviation_v3(
        &self,
        snapshot: &V3PoolSnapshot,
        token_in: Address,
        token_out: Address,
        decimals_in: u8,
        decimals_out: u8,
    ) -> PoolFilterResult {
        // For V3, calculate price from sqrt_price_x96
        // price = (sqrt_price_x96 / 2^96)^2
        let q96 = U256::from(2u128.pow(96));
        let sqrt_price_f64 = snapshot.state.sqrt_price_x96.as_u128() as f64;
        let q96_f64 = q96.as_u128() as f64;
        let price_ratio = (sqrt_price_f64 / q96_f64).powi(2);

        // Determine if zero_for_one
        let zero_for_one = token_in == snapshot.token0;
        let pool_price = if zero_for_one {
            price_ratio // token1 per token0
        } else {
            1.0 / price_ratio // token0 per token1
        };

        // Adjust for decimals
        let decimals_adj = 10f64.powi((decimals_out as i32) - (decimals_in as i32));
        let pool_price_adjusted = pool_price * decimals_adj;

        // Get global reference prices
        let global_price_in = self.global_prices.get(&token_in).copied();
        let global_price_out = self.global_prices.get(&token_out).copied();

        if let (Some(price_in), Some(price_out)) = (global_price_in, global_price_out) {
            let global_price = price_out / price_in;
            let deviation = if global_price > 0.0 {
                ((pool_price_adjusted - global_price) / global_price).abs()
            } else {
                1.0
            };

            let deviation_bps = (deviation * 10000.0) as u32;

            if deviation_bps > self.config.max_price_deviation_bps {
                metrics::increment_pool_filter_price_deviation_too_high();
                return PoolFilterResult {
                    is_valid: false,
                    rejection_reason: Some(format!(
                        "Price deviation too high: {} bps > {} bps",
                        deviation_bps, self.config.max_price_deviation_bps
                    )),
                    effective_liquidity_eth: None,
                    price_deviation_bps: Some(deviation_bps as f64),
                    blocks_since_update: None,
                };
            }

            PoolFilterResult {
                is_valid: true,
                rejection_reason: None,
                effective_liquidity_eth: None,
                price_deviation_bps: Some(deviation_bps as f64),
                blocks_since_update: None,
            }
        } else {
            debug!("No global price available for price deviation check");
            PoolFilterResult {
                is_valid: true,
                rejection_reason: None,
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            }
        }
    }

    /// Check 3: Stale data
    /// Note: This requires block_number to be passed separately or stored in snapshot
    /// For now, we use a time-based estimate, but ideally should use actual block numbers
    fn check_stale_data(&self, last_updated: Instant) -> PoolFilterResult {
        // Estimate blocks since update (Arbitrum: ~0.25s per block = 4 blocks/sec)
        // Using time-based estimate as fallback when block_number is not available
        let age_secs = last_updated.elapsed().as_secs();
        // Conservative estimate: 1 block per second (slower than actual to be safe)
        let blocks_since_update = age_secs;

        if blocks_since_update > self.config.max_stale_blocks {
            metrics::increment_pool_filter_stale_data();
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!(
                    "Data too stale: {} blocks > {} blocks (estimated from {}s age)",
                    blocks_since_update, self.config.max_stale_blocks, age_secs
                )),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: Some(blocks_since_update),
            };
        }

        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: None,
            price_deviation_bps: None,
            blocks_since_update: Some(blocks_since_update),
        }
    }

    /// Check 4: Reserve size vs trade size (V2)
    fn check_reserve_size_v2(
        &self,
        snapshot: &V2PoolSnapshot,
        amount_in: U256,
        token_in: Address,
        _decimals_in: u8,
    ) -> PoolFilterResult {
        let reserve_in = if token_in == snapshot.token0 {
            snapshot.reserve0
        } else {
            snapshot.reserve1
        };

        // Check: reserve_in >= amount_in * multiplier
        let min_reserve = amount_in
            .saturating_mul(U256::from((self.config.min_reserve_multiplier * 1e18) as u128))
            / U256::from(1e18 as u128);

        if reserve_in < min_reserve {
            metrics::increment_pool_filter_reserve_too_small();
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!(
                    "Reserve too small: {} < {} ({}x trade size)",
                    reserve_in, min_reserve, self.config.min_reserve_multiplier
                )),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: None,
            price_deviation_bps: None,
            blocks_since_update: None,
        }
    }

    /// Check 4: Liquidity size vs trade size (V3)
    fn check_liquidity_size_v3(
        &self,
        snapshot: &V3PoolSnapshot,
        amount_in: U256,
        token_in: Address,
        _decimals_in: u8,
    ) -> PoolFilterResult {
        // For V3, we use token balance as proxy
        let balance_in = if token_in == snapshot.token0 {
            snapshot.token0_balance
        } else {
            snapshot.token1_balance
        };

        let min_balance = amount_in
            .saturating_mul(U256::from((self.config.min_reserve_multiplier * 1e18) as u128))
            / U256::from(1e18 as u128);

        if balance_in < min_balance {
            metrics::increment_pool_filter_reserve_too_small();
            return PoolFilterResult {
                is_valid: false,
                rejection_reason: Some(format!(
                    "Balance too small: {} < {} ({}x trade size)",
                    balance_in, min_balance, self.config.min_reserve_multiplier
                )),
                effective_liquidity_eth: None,
                price_deviation_bps: None,
                blocks_since_update: None,
            };
        }

        PoolFilterResult {
            is_valid: true,
            rejection_reason: None,
            effective_liquidity_eth: None,
            price_deviation_bps: None,
            blocks_since_update: None,
        }
    }

    /// Helper: Estimate ETH value of an amount
    /// ✅ MEJORADO: Usa PriceFeed real (CoinGecko/external APIs) para obtener precio de ETH/WETH
    /// No usa fallbacks hardcodeados - siempre requiere precio real desde PriceFeed cache
    fn estimate_eth_value(&self, amount: U256, token: Address, decimals: u8) -> f64 {
        use std::str::FromStr;
        
        // WETH address on Arbitrum
        let weth_address = Address::from_str("0x82af49447d8a07e3bd95bd0d56f35241523fbab1")
            .unwrap_or_else(|_| Address::zero());
        
        // Try to get token price from cache
        if let Some(price_usd) = self.global_prices.get(&token) {
            let amount_f64 = amount.as_u128() as f64 / 10f64.powi(decimals as i32);
            let value_usd = amount_f64 * price_usd;
            
            // ✅ Obtener precio de ETH/WETH desde cache global (viene de PriceFeed/CoinGecko)
            // Si no está en global_prices, intentar usar cache con timestamp (< 24h)
            let eth_price_usd = self.global_prices.get(&weth_address)
                .copied()
                .or_else(|| {
                    // Fallback a cache con timestamp si tiene < 24 horas
                    if let Some((cached_price, cached_time)) = &self.weth_price_cache {
                        let age = cached_time.elapsed();
                        if age < Duration::from_secs(24 * 60 * 60) {
                            Some(*cached_price)
                        } else {
                            warn!("⚠️ WETH price cache expired (>24h), cannot convert to ETH");
                            None
                        }
                    } else {
                        None
                    }
                });
            
            match eth_price_usd {
                Some(price) => value_usd / price,
                None => {
                    // ❌ Sin precio de ETH disponible - retornar 0 en lugar de fallback hardcodeado
                    warn!("⚠️ Cannot convert to ETH: WETH price not available (neither in global_prices nor valid cache). Token: {:?}, Amount: {}", token, amount);
                    0.0 // Retornar 0 en lugar de fallback hardcodeado
                }
            }
        } else {
            // Token price not available - cannot estimate ETH value
            warn!("⚠️ Cannot estimate ETH value: token price not available for {:?}", token);
            0.0
        }
    }
}

