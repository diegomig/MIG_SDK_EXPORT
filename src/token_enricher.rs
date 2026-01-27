use crate::cache::CacheManager;
use crate::contracts::Erc20;
use crate::database::{batch_upsert_tokens, upsert_token_relation, DbPool};
use crate::multicall::{Call, Multicall};
use crate::settings::Settings;
use anyhow::Result;
use ethers::types::Address;
use log::{info, warn};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

/// Token type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Stablecoin,
    Wrapped,
    Native,
    LP,
    Synthetic,
    Other,
}

impl TokenType {
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            TokenType::Stablecoin => "stablecoin",
            TokenType::Wrapped => "wrapped",
            TokenType::Native => "native",
            TokenType::LP => "lp",
            TokenType::Synthetic => "synthetic",
            TokenType::Other => "other",
        }
    }
}

pub struct TokenEnricher;

impl TokenEnricher {
    /// Enrich tokens with on-chain metadata (symbol, decimals) and classify them
    pub async fn run<M: ethers::providers::Middleware + 'static>(
        provider: Arc<M>,
        multicall: Arc<Multicall<M>>,
        cache: Arc<CacheManager>,
        db: &DbPool,
        tokens: &[Address],
        _settings: &Settings,
    ) -> Result<()> {
        // ALWAYS fetch from chain for tokens that don't have symbols in DB
        // Ignore cache to ensure DB gets updated properly
        let mut to_fetch: Vec<Address> = Vec::new();
        let mut seen: HashSet<Address> = HashSet::new();

        for &t in tokens {
            if seen.insert(t) {
                to_fetch.push(t);
            }
        }

        if to_fetch.is_empty() {
            info!("TokenEnricher: No tokens to process");
            return Ok(());
        }

        info!(
            "TokenEnricher: Fetching metadata for {} tokens",
            to_fetch.len()
        );

        let erc20 = Erc20::new(Address::zero(), provider.clone());
        let decimals_fn = erc20.abi().function("decimals")?;
        let symbol_fn = erc20.abi().function("symbol")?;

        let mut calls: Vec<Call> = Vec::with_capacity(to_fetch.len() * 2);
        for token in &to_fetch {
            calls.push(Call {
                target: *token,
                call_data: erc20.decimals().calldata().unwrap(),
            });
            calls.push(Call {
                target: *token,
                call_data: erc20.symbol().calldata().unwrap(),
            });
        }

        let results = multicall.run(calls, None).await?;

        // Decode pairwise (decimals, symbol) and classify
        let mut upserts = Vec::with_capacity(to_fetch.len());
        let mut enriched_count = 0;
        let total_to_fetch = to_fetch.len();

        for (i, token) in to_fetch.into_iter().enumerate() {
            let dec_raw = &results[i * 2];
            let sym_raw = &results[i * 2 + 1];
            let decimals = decimals_fn
                .decode_output(dec_raw)
                .ok()
                .and_then(|v| v.get(0).and_then(|x| x.clone().into_uint()))
                .and_then(|u| u.try_into().ok());
            let symbol = symbol_fn
                .decode_output(sym_raw)
                .ok()
                .and_then(|v| v.get(0).and_then(|x| x.clone().into_string()));

            if let Some(d) = decimals {
                cache.token_decimals_cache.insert(token, d);
                enriched_count += 1;
            }

            // Classify token based on symbol and address
            let token_type = Self::classify_token(&token, symbol.as_deref(), decimals);

            // Store classification in DB (would need to extend batch_upsert_tokens to accept token_type)
            upserts.push((token, symbol.clone(), decimals));

            // Detect and store token relations (e.g., WETH <-> ETH)
            if let Some(ref sym) = symbol {
                Self::detect_relations(db, token, sym, token_type)
                    .await
                    .ok();
            }
        }

        batch_upsert_tokens(db, upserts).await?;

        // Update cache size metrics after inserting decimals
        cache.record_cache_sizes().await;

        info!(
            "TokenEnricher: Enriched {}/{} tokens successfully",
            enriched_count, total_to_fetch
        );
        Ok(())
    }

    /// Classify token based on symbol, address, and decimals
    fn classify_token(address: &Address, symbol: Option<&str>, _decimals: Option<u8>) -> TokenType {
        // Known addresses on Arbitrum
        let addr_str = format!("{:?}", address).to_lowercase();

        // Native/Wrapped
        if addr_str == "0x82af49447d8a07e3bd95bd0d56f35241523fbab1" {
            return TokenType::Wrapped; // WETH
        }

        // Stablecoins
        if let Some(sym) = symbol {
            let sym_upper = sym.to_uppercase();
            if sym_upper.contains("USD")
                || sym_upper == "USDC"
                || sym_upper == "USDT"
                || sym_upper == "DAI"
                || sym_upper == "BUSD"
                || sym_upper == "FRAX"
            {
                return TokenType::Stablecoin;
            }

            // Wrapped tokens
            if sym_upper.starts_with('W') && sym.len() <= 5 {
                return TokenType::Wrapped;
            }

            // LP tokens
            if sym_upper.contains("LP") || sym_upper.contains("-") || sym_upper.contains("/") {
                return TokenType::LP;
            }
        }

        // Default
        TokenType::Other
    }

    /// Detect and store token relations (wrap, bridge, lp_underlying)
    async fn detect_relations(
        db: &DbPool,
        token: Address,
        symbol: &str,
        _token_type: TokenType,
    ) -> Result<()> {
        // Known wrapped relationships on Arbitrum
        let weth = Address::from_str("0x82af49447d8a07e3bd95bd0d56f35241523fbab1").unwrap();

        if symbol == "WETH" || token == weth {
            // WETH wraps ETH (native)
            upsert_token_relation(
                db,
                Address::zero(), // ETH (native)
                token,
                "wrap",
                Some("on-chain"),
                Some(1.0),
            )
            .await
            .ok();
        }

        // Detect bridged tokens (e.g., USDC.e, WBTC.e)
        if symbol.ends_with(".e") {
            let base_symbol = symbol.trim_end_matches(".e");
            // Would need to lookup base token by symbol
            info!(
                "TokenEnricher: Detected bridged token {} (base: {})",
                symbol, base_symbol
            );
        }

        Ok(())
    }

    /// Run token enrichment periodically (call this from a background task)
    pub async fn run_periodic<M: ethers::providers::Middleware + 'static>(
        provider: Arc<M>,
        multicall: Arc<Multicall<M>>,
        cache: Arc<CacheManager>,
        db: DbPool,
        settings: Arc<Settings>,
        interval_hours: u64,
    ) -> Result<()> {
        use tokio::time::{sleep, Duration};

        loop {
            // Fetch tokens that need enrichment from DB
            let tokens_to_enrich = match crate::database::get_tokens_without_symbols(&db, 100).await
            {
                Ok(addrs) => addrs,
                Err(e) => {
                    warn!(
                        "TokenEnricher: Failed to fetch tokens for enrichment: {}",
                        e
                    );
                    vec![]
                }
            };

            if !tokens_to_enrich.is_empty() {
                info!(
                    "TokenEnricher: Starting periodic enrichment of {} tokens",
                    tokens_to_enrich.len()
                );
                if let Err(e) = Self::run(
                    provider.clone(),
                    multicall.clone(),
                    cache.clone(),
                    &db,
                    &tokens_to_enrich,
                    &settings,
                )
                .await
                {
                    warn!("TokenEnricher: Periodic run failed: {}", e);
                }
            } else {
                info!("TokenEnricher: No tokens to enrich, skipping cycle");
            }

            sleep(Duration::from_secs(interval_hours * 3600)).await;
        }
    }
}
