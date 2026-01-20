// src/price_feeds.rs

use ethers::prelude::{abigen, Address, Middleware, BlockId};
use ethers::types::{BlockNumber, I256};
use ethers::types::U256;
use ethers::providers::RawCall;
use ethers::abi::Token;
use std::collections::HashMap;
use anyhow::Result;
use log::{warn, info};
use dashmap::DashMap;
use std::sync::Arc;
use crate::flight_recorder::FlightRecorder;
use crate::{record_phase_start, record_phase_end};
use crate::multicall::{Multicall, Call};
use crate::cache::CacheManager;
use crate::contracts::IUniswapV3Factory;
use crate::background_price_updater::SharedPriceCache;

abigen!(
    AggregatorV3Interface,
    r#"[
        function latestRoundData() external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
        function decimals() external view returns (uint8)
    ]"#,
);

abigen!(
    ERC20Minimal,
    r#"[
        function decimals() external view returns (uint8)
    ]"#,
);

use std::time::{Duration, Instant};

/// Cached price entry with block number for invalidation.
///
/// Prices are cached per block number to ensure consistency. Cache entries are invalidated
/// when the block number changes.
#[derive(Debug, Clone)]
pub struct PriceEntry {
    /// USD price of the token
    pub price: f64,
    /// Block number at which this price was fetched
    pub block_number: u64,
}

/// Price feed aggregator for USD price data.
///
/// Aggregates prices from multiple sources (Chainlink, Uniswap V3, external APIs) with
/// intelligent fallback and caching strategies.
///
/// ## Features
///
/// - **Multi-Source Aggregation**: Chainlink oracles, Uniswap V3 pools, external APIs
/// - **Block-Based Caching**: Cache invalidation by block number
/// - **Fallback Strategy**: Automatic fallback to alternative sources
/// - **Price Deviation Checks**: Validates prices against tolerance thresholds
///
/// ## Usage
///
/// ```rust
/// let price_feed = PriceFeed::new(
///     provider,
///     oracle_addresses,
///     uniswap_v3_factory,
///     cache_ttl_seconds,
///     multicall_address,
///     batch_size,
///     cache_manager,
///     anchor_tokens,
///     enable_twap_fallback,
///     price_deviation_tolerance_bps,
/// );
/// let prices = price_feed.get_usd_prices_batch(&tokens, None).await?;
/// ```
#[derive(Debug, Clone)]
pub struct PriceFeed<M: Middleware> {
    provider: Arc<M>,
    oracle_addresses: HashMap<Address, Address>,
    /// Cache de decimales por oracle de Chainlink (key = address del oracle)
    /// Evita hardcodear 8 decimales, que rompe el sizing cuando el feed usa 18.
    oracle_decimals_cache: Arc<DashMap<Address, u8>>,
    // FASE 4.1: Replaced Mutex<LruCache> with DashMap for lock-free access
    // Riesgo Crítico 3: Cache por block number (no TTL) - invalidación estricta por bloque
    pub price_cache: Arc<DashMap<Address, PriceEntry>>,
    // This cache is for historical mode, keyed by block number
    pub historical_price_cache: Arc<DashMap<(Address, u64), f64>>,
    pub token_decimals_cache: Arc<DashMap<Address, u8>>,
    // Riesgo Crítico 3: Mantener current_block para invalidación estricta
    current_block: Arc<std::sync::atomic::AtomicU64>,
    multicall_address: Address,
    multicall_batch_size: usize,
    cache_manager: Arc<CacheManager>,
    anchor_tokens: Vec<Address>,
    uniswap_v3_factory: IUniswapV3Factory<M>,
    enable_twap_fallback: bool,
    price_deviation_tolerance_bps: u32,
    // FASE 4.1: Max cache sizes for manual eviction
    price_cache_max_size: usize,
    historical_cache_max_size: usize,
    // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    flight_recorder: Option<Arc<FlightRecorder>>,
    // ✅ P1 OPTIMIZATION: Parallel price fetching configuration
    parallel_price_fetching_enabled: bool,
    price_fetch_chunk_size: usize,
}

impl<M: Middleware> PriceFeed<M> where M: 'static {
    pub fn new(
        provider: Arc<M>,
        oracle_addresses: HashMap<Address, Address>,
        uniswap_v3_factory_address: Address,
        cache_ttl_seconds: u64,
        multicall_address: Address,
        multicall_batch_size: usize,
        cache_manager: Arc<CacheManager>,
        anchor_tokens: Vec<Address>,
        enable_twap_fallback: bool,
        price_deviation_tolerance_bps: u32,
    ) -> Self {
        Self {
            provider: Arc::clone(&provider),
            oracle_addresses,
            oracle_decimals_cache: Arc::new(DashMap::new()),
            // FASE 4.1: Lock-free DashMap (max 100 entries, manual eviction)
            price_cache: Arc::new(DashMap::new()),
            // FASE 4.1: Lock-free DashMap (max 500 entries, manual eviction)
            historical_price_cache: Arc::new(DashMap::new()),
            token_decimals_cache: Arc::new(DashMap::new()),
            // Riesgo Crítico 3: Inicializar current_block en 0 (se actualizará cuando se conozca el block actual)
            current_block: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            multicall_address,
            multicall_batch_size,
            cache_manager,
            anchor_tokens,
            uniswap_v3_factory: IUniswapV3Factory::new(uniswap_v3_factory_address, provider),
            enable_twap_fallback,
            price_deviation_tolerance_bps,
            price_cache_max_size: 100,
            historical_cache_max_size: 500,
            flight_recorder: None,
            // ✅ P1 OPTIMIZATION: Default parallel fetching disabled (can be enabled via with_settings)
            parallel_price_fetching_enabled: false,
            price_fetch_chunk_size: 20,
        }
    }
    
    /// ✅ P1 OPTIMIZATION: Configure parallel price fetching settings
    pub fn with_parallel_fetching(mut self, enabled: bool, chunk_size: usize) -> Self {
        self.parallel_price_fetching_enabled = enabled;
        self.price_fetch_chunk_size = chunk_size;
        self
    }
    
    /// Set flight recorder for instrumentation
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder);
        self
    }

    pub fn oracle_count(&self) -> usize {
        self.oracle_addresses.len()
    }

    /// Riesgo Crítico 3: Actualizar block number actual para invalidación estricta
    /// OPTIMIZACIÓN FASE 4: Mantener cache del bloque anterior para fallback rápido
    /// Optimizado: usa retain() que es más eficiente que iterar y remove() individualmente
    pub fn update_current_block(&self, block_number: u64) {
        self.current_block.store(block_number, std::sync::atomic::Ordering::Relaxed);
        // Invalidar entradas de cache que no son del bloque actual NI del bloque anterior
        // Mantener cache del bloque anterior permite fallback rápido si el fetch actual tarda
        // Usar retain() es más eficiente que iterar y remove() individualmente
        let before_len = self.price_cache.len();
        let prev_block = if block_number > 0 { block_number - 1 } else { 0 };
        self.price_cache.retain(|_, entry| {
            entry.block_number == block_number || entry.block_number == prev_block
        });
        let invalidated = before_len - self.price_cache.len();
        if invalidated > 0 {
            log::debug!("Invalidated {} price cache entries for block {} (kept current + previous block, cache size: {})", invalidated, block_number, self.price_cache.len());
        }
    }

    /// Convenience: live price (no historical block)
    pub async fn get_usd_price(&self, token_address: Address) -> Result<f64> {
        self.get_usd_price_at(token_address, None).await
    }

    /// Get price at optional historical block
    pub async fn get_usd_price_at(&self, token_address: Address, block: Option<BlockId>) -> Result<f64> {
        let map = self.get_usd_prices_batch(&[token_address], block).await?;
        let price = map.get(&token_address)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("No price resolved for token {:?}", token_address))?;
        Ok(price)
    }

    /// Get USD prices for a batch of tokens.
    ///
    /// Notes:
    /// - ✅ FIX: Chainlink timeout set to 150ms with parallel pool fallback execution.
    ///   Chainlink and pool fallback run in parallel for optimal latency (150ms max).
    /// - For background refreshers, use `get_usd_prices_batch_with_chainlink_timeout` to allow a longer timeout
    ///   without increasing RPC/CU usage (it still uses the same multicall/batching).
    pub async fn get_usd_prices_batch(&self, tokens: &[Address], block: Option<BlockId>) -> Result<HashMap<Address, f64>> {
        self.get_usd_prices_batch_inner(tokens, block, Duration::from_millis(150), None).await
    }

    /// Same as `get_usd_prices_batch`, but allows passing SharedPriceCache for anchor token fallback.
    /// This is used in the hot path when SharedPriceCache is available.
    /// Chainlink and pool fallback execute in parallel for optimal latency.
    pub async fn get_usd_prices_batch_with_shared_cache(
        &self,
        tokens: &[Address],
        block: Option<BlockId>,
        shared_price_cache: Option<&SharedPriceCache>,
    ) -> Result<HashMap<Address, f64>> {
        // ✅ OPTIMIZACIÓN LATENCIA: Aumentar timeout de Chainlink de 150ms a 200ms para mejorar success rate
        self.get_usd_prices_batch_inner(tokens, block, Duration::from_millis(200), shared_price_cache).await
    }

    /// Same as `get_usd_prices_batch`, but allows overriding the internal Chainlink timeout.
    /// This is intended for background components where waiting longer is fine (and avoids "missing=1" forever).
    pub async fn get_usd_prices_batch_with_chainlink_timeout(
        &self,
        tokens: &[Address],
        block: Option<BlockId>,
        chainlink_timeout: Duration,
    ) -> Result<HashMap<Address, f64>> {
        self.get_usd_prices_batch_inner(tokens, block, chainlink_timeout, None).await
    }
    
    /// Same as `get_usd_prices_batch_with_chainlink_timeout`, but allows passing SharedPriceCache.
    pub async fn get_usd_prices_batch_with_chainlink_timeout_and_cache(
        &self,
        tokens: &[Address],
        block: Option<BlockId>,
        chainlink_timeout: Duration,
        shared_price_cache: Option<&SharedPriceCache>,
    ) -> Result<HashMap<Address, f64>> {
        self.get_usd_prices_batch_inner(tokens, block, chainlink_timeout, shared_price_cache).await
    }

    async fn get_usd_prices_batch_inner(
        &self,
        tokens: &[Address],
        block: Option<BlockId>,
        chainlink_timeout: Duration,
        shared_price_cache: Option<&SharedPriceCache>,
    ) -> Result<HashMap<Address, f64>> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de price fetch
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "price_fetch_internal", serde_json::json!({
                "tokens_count": tokens.len(),
                "has_shared_cache": shared_price_cache.is_some()
            }));
        }
        
        let mut results = HashMap::new();
        let mut tokens_to_fetch = Vec::new();

        // ✅ FIX: Usar try_into() para evitar panic por overflow
        let block_number = if let Some(BlockId::Number(BlockNumber::Number(num_u256))) = block { 
            num_u256.try_into().unwrap_or_else(|_| {
                log::warn!("⚠️ Block number too large for u64, using 0");
                0u64
            })
        } else { 
            0 
        };
        let is_historical = block.is_some() && block_number > 0;

        // FASE 4.1: Lock-free cache check
        if is_historical {
            for &token in tokens {
                if let Some(price) = self.historical_price_cache.get(&(token, block_number)) {
                    results.insert(token, *price.value());
                } else {
                    tokens_to_fetch.push(token);
                }
            }
        } else {
            // OPTIMIZACIÓN CRÍTICA FASE 4: Usar cache del bloque actual O del bloque anterior
            // Los precios de Chainlink no cambian significativamente entre bloques consecutivos (250ms)
            // Es mejor usar un precio de hace 250ms que esperar 2-3s por un RPC
            // FASE 4: Mantenemos cache del bloque anterior explícitamente para fallback rápido
            let current_block = self.current_block.load(std::sync::atomic::Ordering::Relaxed);
            let prev_block = if current_block > 0 { current_block - 1 } else { 0 };
            let mut cache_hits = 0;
            let mut cache_misses = 0;
            for &token in tokens {
                if let Some(entry) = self.price_cache.get(&token) {
                    // Cache válido si es del bloque actual O del bloque anterior
                    // FASE 4: Mantenemos cache del bloque anterior explícitamente
                    if entry.block_number == current_block || entry.block_number == prev_block {
                        // ✅ CRITICAL FIX: Si el precio en cache es 0, intentar fetch (puede ser un error previo)
                        if entry.price > 0.0 {
                            // Usar precio del bloque actual o anterior (suficientemente fresco)
                            results.insert(token, entry.price);
                            cache_hits += 1;
                            if entry.block_number == current_block {
                                log::debug!("✅ Cache HIT for token {:?} from current block {}", token, current_block);
                            } else {
                                log::debug!("✅ Cache HIT for token {:?} from previous block {} (current: {})", token, entry.block_number, current_block);
                            }
                        } else {
                            // Precio en cache es 0, intentar fetch de nuevo (puede ser un error previo)
                            tokens_to_fetch.push(token);
                            cache_misses += 1;
                            log::debug!("⚠️ Cache has price=0 for token {:?}, will retry fetch", token);
                        }
                    } else {
                        tokens_to_fetch.push(token);
                        cache_misses += 1;
                    }
                } else {
                    tokens_to_fetch.push(token);
                    cache_misses += 1;
                }
            }
            if cache_hits > 0 {
                log::info!("📦 Cache stats: {} hits, {} misses for {} tokens (current_block: {})", cache_hits, cache_misses, tokens.len(), current_block);
            }
        }

        tokens_to_fetch.sort();
        tokens_to_fetch.dedup();

        if tokens_to_fetch.is_empty() {
            // ✅ FLIGHT RECORDER: Registrar fin de price fetch (todos en cache)
            if let Some(ref recorder) = self.flight_recorder {
                record_phase_end!(recorder, "price_fetch_internal", start_time, serde_json::json!({
                    "prices_fetched": results.len(),
                    "tokens_requested": tokens.len(),
                    "all_from_cache": true
                }));
            }
            return Ok(results);
        }

        tokens_to_fetch.retain(|t| *t != Address::zero());
        if tokens_to_fetch.is_empty() {
            return Ok(results);
        }

        // ✅ OPTIMIZACIÓN: Consultar SharedPriceCache PRIMERO para preparar known_prices
        // Esto permite ejecutar Chainlink y Pool Fallback en PARALELO con anchor tokens ya disponibles
        let mut known_prices = results.clone();
        let mut shared_cache_used = 0;
        if let Some(shared_cache) = shared_price_cache {
            // Consultar anchor tokens PRIMERO (necesarios para pool fallback)
            for &anchor in &self.anchor_tokens {
                if !known_prices.contains_key(&anchor) {
                    if let Some(price) = shared_cache.get_price(&anchor) {
                        if price > 0.0 {
                            known_prices.insert(anchor, price);
                            shared_cache_used += 1;
                            log::debug!("✅ Using anchor price from SharedPriceCache: {:?} = ${:.2}", anchor, price);
                        }
                    }
                }
            }
            
            // Consultar tokens solicitados que ya están en SharedPriceCache (entry tokens, etc.)
            for &token in &tokens_to_fetch {
                if !known_prices.contains_key(&token) {
                    if let Some(price) = shared_cache.get_price(&token) {
                        if price > 0.0 {
                            known_prices.insert(token, price);
                            shared_cache_used += 1;
                            log::debug!("✅ Using token price from SharedPriceCache: {:?} = ${:.2}", token, price);
                        }
                    }
                }
            }
            
            if shared_cache_used > 0 {
                log::info!("  📊 SharedPriceCache provided {} token prices (anchor + requested) for parallel execution", shared_cache_used);
            }
        } else {
            log::debug!("  ℹ️ SharedPriceCache not available, relying on Chainlink and internal cache only");
        }
        
        // Determinar si podemos hacer pool fallback (si tenemos al menos un precio de anchor token)
        let can_do_pool_fallback = self.anchor_tokens.iter().any(|&anchor| {
            known_prices.contains_key(&anchor)
        });
        
        // ✅ Log explícito cuando SharedPriceCache habilita pool fallback
        if can_do_pool_fallback && shared_cache_used > 0 {
            let anchor_count = self.anchor_tokens.iter()
                .filter(|&a| known_prices.contains_key(a))
                .count();
            log::info!("  ✅ Pool fallback ENABLED via SharedPriceCache ({} anchor prices found)", anchor_count);
        }
        
        // Determinar tokens que aún necesitan precio
        let tokens_still_needed: Vec<Address> = tokens_to_fetch.iter()
            .filter(|t| !known_prices.contains_key(t))
            .copied()
            .collect();
        
        // ✅ FIX: Use the provided chainlink_timeout parameter instead of hardcoded constant
        // This allows callers (like graph_service.rs) to specify longer timeouts for background operations
        let total_budget = chainlink_timeout; // Use the provided timeout as total budget
        let fetch_start = Instant::now();
        let mut chainlink_prices = HashMap::new();
        let mut pool_fallback_prices = HashMap::new();
        
        if !tokens_still_needed.is_empty() {
            // ✅ FIX: Use 80% of total budget for Chainlink, leaving 20% for pool fallback
            // For short timeouts (150ms), this gives ~120ms for Chainlink and ~30ms for fallback
            // For long timeouts (2000ms), this gives ~1600ms for Chainlink and ~400ms for fallback
            let remaining = total_budget.saturating_sub(fetch_start.elapsed());
            let chainlink_timeout_80pct = Duration::from_millis((chainlink_timeout.as_millis() as f64 * 0.8) as u64);
            let chainlink_timeout_actual = remaining.min(chainlink_timeout_80pct);
            
            let chainlink_result = if chainlink_timeout_actual > Duration::from_millis(10) {
                log::info!("  🔗 Attempting Chainlink fetch for {} tokens with timeout {:?}", tokens_still_needed.len(), chainlink_timeout_actual);
                tokio::time::timeout(
                    chainlink_timeout_actual,
                    self.fetch_from_chainlink(&tokens_still_needed, block)
                ).await
            } else {
                log::warn!("  ⚠️ No time budget for Chainlink (timeout: {:?}), skipping", chainlink_timeout_actual);
                // No hay tiempo suficiente para Chainlink - simular timeout
                // Usar un timeout muy corto para generar Elapsed naturalmente
                tokio::time::timeout(
                    Duration::from_millis(1),
                    async { Err(anyhow::anyhow!("No time budget for Chainlink")) }
                ).await
            };
            
            // Procesar resultado de Chainlink
            match chainlink_result {
                Ok(Ok(prices)) => {
                    chainlink_prices = prices;
                    let fetch_duration = fetch_start.elapsed();
                    if !chainlink_prices.is_empty() {
                        log::info!("✅ Chainlink fetch completed in {:?}, got {} prices (out of {} requested)", fetch_duration, chainlink_prices.len(), tokens_still_needed.len());
                    } else {
                        log::warn!("⚠️ Chainlink fetch completed in {:?} but returned 0 prices (requested {} tokens). This may indicate missing oracle configuration.", fetch_duration, tokens_still_needed.len());
                    }
                }
                Ok(Err(e)) => {
                    log::warn!("⚠️ Chainlink fetch failed: {:?}", e);
                }
                Err(_) => {
                    log::warn!("⚠️ Chainlink fetch timeout after {:?} (requested {} tokens)", chainlink_timeout_actual, tokens_still_needed.len());
                }
            }
        }
        
        // ✅ FASE 3.1: Pool fallback solo para lo que realmente falta (con tiempo restante)
        let still_missing: Vec<_> = tokens_still_needed.iter()
            .filter(|t| !chainlink_prices.contains_key(t))
            .copied()
            .collect();
        
        log::info!("  📊 Price fetch status: {} tokens still missing, can_do_pool_fallback={}, known_prices has {} anchor tokens", 
                   still_missing.len(), can_do_pool_fallback, 
                   self.anchor_tokens.iter().filter(|&a| known_prices.contains_key(a)).count());
        
        if !still_missing.is_empty() && can_do_pool_fallback {
            // ✅ FIX: Use remaining time from total budget for pool fallback
            let remaining = total_budget.saturating_sub(fetch_start.elapsed());
            // Use at least 20% of original timeout for fallback, but cap at 500ms for very long timeouts
            let fallback_timeout_20pct = Duration::from_millis((chainlink_timeout.as_millis() as f64 * 0.2) as u64);
            let fallback_timeout = remaining.min(fallback_timeout_20pct.min(Duration::from_millis(500)));
            
            if fallback_timeout > Duration::from_millis(20) {
                log::info!("  🔄 Attempting pool fallback for {} tokens with timeout {:?} (known_prices has {} prices)", 
                           still_missing.len(), fallback_timeout, known_prices.len());
                match tokio::time::timeout(
                    fallback_timeout,
                    self.fetch_from_twap_fallback(&still_missing, block, &known_prices)
                ).await {
                    Ok(Ok(fallback_prices)) => {
                        pool_fallback_prices = fallback_prices;
                        let fetch_duration = fetch_start.elapsed();
                        log::info!("✅ Pool fallback completed in {:?}, got {} prices (out of {} requested)", 
                                  fetch_duration, pool_fallback_prices.len(), still_missing.len());
                    }
                    Ok(Err(e)) => {
                        log::warn!("⚠️ Pool fallback failed: {:?}", e);
                    }
                    Err(_) => {
                        log::warn!("⚠️ Pool fallback timeout after {:?} (requested {} tokens)", fallback_timeout, still_missing.len());
                    }
                }
            } else {
                log::warn!("  ⚠️ Pool fallback skipped: timeout too short ({:?}ms, need >20ms)", fallback_timeout.as_millis());
            }
        } else {
            if still_missing.is_empty() {
                log::info!("  ℹ️ Pool fallback skipped: no tokens missing (all prices obtained from Chainlink/SharedPriceCache)");
            } else if !can_do_pool_fallback {
                log::warn!("  ⚠️ Pool fallback skipped: no anchor tokens in known_prices (need at least one anchor token for pool fallback)");
            }
        }
        
        // Merge known_prices con Chainlink (Chainlink tiene prioridad)
        known_prices.extend(chainlink_prices.clone());

        // Merge results: Chainlink primero, luego cache del bloque anterior, luego pool fallback
        // FASE 4: Fallback para stablecoins usando precio hardcoded = 1.0 si no se encuentra
        // Nota: `.unwrap()` está prohibido fuera de tests (ver `docs/code_conventions.md`).
        let parse_addr = |label: &'static str, s: &'static str| -> Option<Address> {
            match s.parse::<Address>() {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("⚠️ Invalid address constant ({}={}): {:?}", label, s, e);
                    None
                }
            }
        };

        // Arbitrum One stablecoins
        let usdc_native = parse_addr("USDC_NATIVE", "0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
        // Correct USDC.e on Arbitrum One:
        let usdc_e = parse_addr("USDC_E", "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8");
        let usdt = parse_addr("USDT", "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");
        // WETH on Arbitrum One (fallback final si todas las fuentes fallan)
        let weth = parse_addr("WETH", "0x82af49447d8a07e3bd95bd0d56f35241523fBab1");
        
        for token in &tokens_to_fetch {
            let chainlink_price = chainlink_prices.get(token).copied().unwrap_or(0.0);
            let pool_price = pool_fallback_prices.get(token).copied().unwrap_or(0.0);

            if chainlink_price > 0.0 {
                results.insert(*token, chainlink_price);
                log::debug!("  ✅ Using Chainlink price for token {:?}: ${:.2}", token, chainlink_price);
                continue;
            }

            if pool_price > 0.0 {
                results.insert(*token, pool_price);
                log::info!(
                    "  🔄 Using pool-calculated price for token {:?}: ${:.2} (Chainlink missing/invalid, calculated from pools)",
                    token,
                    pool_price
                );
                continue;
            }

            // FASE 4: Fallback hardcoded para stablecoins si todo falla
            let is_usdc_e = usdc_e.is_some_and(|a| *token == a);
            let is_usdc_native = usdc_native.is_some_and(|a| *token == a);
            let is_usdt = usdt.is_some_and(|a| *token == a);

            if is_usdc_e {
                // Fallback adicional: usar precio de USDC nativo si está disponible
                if let Some(usdc_native_addr) = usdc_native {
                    if let Some(usdc_price) = chainlink_prices.get(&usdc_native_addr).copied().filter(|p| *p > 0.0) {
                        log::info!("  🔄 Using USDC (native) price as fallback for USDC.e: ${:.2}", usdc_price);
                        results.insert(*token, usdc_price);
                        continue;
                    } else if let Some(usdc_price) = results.get(&usdc_native_addr).copied().filter(|p| *p > 0.0) {
                        log::info!("  🔄 Using USDC (native) price from cache as fallback for USDC.e: ${:.2}", usdc_price);
                        results.insert(*token, usdc_price);
                        continue;
                    }
                }

                // Último recurso: hardcoded $1.00
                // Validación de normalización: precio debe ser finito y positivo
                let hardcoded_price: f64 = 1.0;
                if hardcoded_price.is_finite() && hardcoded_price > 0.0 {
                    results.insert(*token, hardcoded_price);
                    log::info!("  💰 Using hardcoded price $1.00 for USDC.e (all fallbacks failed)");
                } else {
                    log::warn!("  ⚠️ Hardcoded price for USDC.e failed normalization check, skipping");
                }
                continue;
            }

            if is_usdc_native || is_usdt {
                // USDC y USDT siempre valen ~1 USD
                // Validación de normalización: precio debe ser finito y positivo
                let hardcoded_price: f64 = 1.0;
                if hardcoded_price.is_finite() && hardcoded_price > 0.0 {
                    results.insert(*token, hardcoded_price);
                    log::info!("  💰 Using hardcoded price $1.00 for stablecoin {:?} (Chainlink and pool fallback failed)", token);
                } else {
                    log::warn!("  ⚠️ Hardcoded price for stablecoin {:?} failed normalization check, skipping", token);
                }
                continue;
            }

            // Fallback final para WETH: usar precio hardcoded si todas las fuentes fallan
            // NOTA: Este es un último recurso. El precio de WETH varía mucho, pero es mejor que 0.0
            let is_weth = weth.is_some_and(|a| *token == a);
            if is_weth {
                // Precio hardcoded conservador para WETH (último recurso)
                // Este valor debería ser actualizado periódicamente o mejor aún, usar SharedPriceCache
                // Por ahora usamos un valor razonable basado en el precio típico de ETH (~$3000-4000)
                // Si SharedPriceCache está disponible, debería haber proporcionado el precio antes
                let weth_hardcoded_price: f64 = 3500.0; // Precio conservador de fallback
                // Validación de normalización: precio debe ser finito, positivo y dentro de rango razonable
                if weth_hardcoded_price.is_finite() 
                    && weth_hardcoded_price > 0.0 
                    && weth_hardcoded_price <= 100000.0 // Límite superior razonable para ETH
                {
                    results.insert(*token, weth_hardcoded_price);
                    log::warn!(
                        "  ⚠️ Using hardcoded fallback price ${:.2} for WETH {:?} (all sources failed - Chainlink timeout, no pool fallback, no SharedPriceCache)",
                        weth_hardcoded_price,
                        token
                    );
                } else {
                    log::warn!("  ⚠️ Hardcoded WETH price failed normalization check (value: {}), skipping", weth_hardcoded_price);
                }
                continue;
            }

            // ✅ FASE 2.3: Explicit error - no retornar 0.0 silenciosamente
            // Si todas las fuentes fallan, retornar error explícito
            log::warn!("  ❌ FASE 2.3: All price sources failed for token {:?} (Chainlink=0, pool=0, no hardcoded fallback)", token);
            // No agregar a results - el error se manejará al final
        }
        
        // ✅ FIX: Return partial results instead of error when some tokens fail
        // This allows graph_service to use whatever prices were successfully fetched
        let missing_tokens: Vec<Address> = tokens_to_fetch.iter()
            .filter(|t| !results.contains_key(t))
            .copied()
            .collect();
        
        if !missing_tokens.is_empty() {
            // Log warning but don't fail - return partial results
            log::warn!(
                "⚠️ FASE 2.3: Price feed failed for {} tokens (out of {} requested): Chainlink, pool fallback, and hardcoded fallbacks all failed. Missing tokens: {:?}. Returning {} partial prices.",
                missing_tokens.len(),
                tokens_to_fetch.len(),
                missing_tokens,
                results.len()
            );
            // Continue and return partial results instead of error
        }

        // Resumen de fuentes de precio utilizadas
        if !results.is_empty() {
            let chainlink_found = chainlink_prices.len();
            let pool_found = pool_fallback_prices.len();
            let total_found = results.len();
            log::info!(
                "  📊 Price fetch summary: {} prices found (Chainlink: {}, Pool fallback: {}, Cache/SharedPriceCache/Hardcoded: {})",
                total_found,
                chainlink_found,
                pool_found,
                total_found.saturating_sub(chainlink_found).saturating_sub(pool_found)
            );
        }

        // FASE 4.1: Lock-free cache insert
        if is_historical {
            for (token, price) in &results {
                self.historical_price_cache.insert((*token, block_number), *price);
                // Manual eviction if cache exceeds max size
                if self.historical_price_cache.len() > self.historical_cache_max_size {
                    // Remove oldest 10% of entries (simple eviction)
                    let to_remove = self.historical_price_cache.len() - self.historical_cache_max_size;
                    let mut removed = 0;
                    for entry in self.historical_price_cache.iter() {
                        if removed >= to_remove {
                            break;
                        }
                        self.historical_price_cache.remove(entry.key());
                        removed += 1;
                    }
                }
            }
        } else {
            // ✅ FASE 2.3: Never cache 0.0 prices
            // Riesgo Crítico 3: Almacenar con block number actual (no timestamp)
            // 🚀 RPC OPTIMIZATION: No hacer get_block_number() aquí - usar current_block directamente
            // Si current_block es 0, usar 0 (será invalidado en el próximo bloque cuando se actualice)
            // Esto evita una llamada RPC adicional
            let current_block = self.current_block.load(std::sync::atomic::Ordering::Relaxed);
            let block_to_store = current_block; // Usar directamente, sin fallback a get_block_number()
            
            // ✅ FASE 2.3: Only cache prices > 0.0
            for (token, price) in &results {
                if *price > 0.0 {
                    self.price_cache.insert(*token, PriceEntry { price: *price, block_number: block_to_store });
                }
            }
            
            // Manual eviction if cache exceeds max size
            if self.price_cache.len() > self.price_cache_max_size {
                // Remove oldest 10% of entries (simple eviction)
                let to_remove = self.price_cache.len() - self.price_cache_max_size;
                let mut removed = 0;
                for entry in self.price_cache.iter() {
                    if removed >= to_remove {
                        break;
                    }
                    self.price_cache.remove(entry.key());
                    removed += 1;
                }
            }
        }

        let duration = start_time.elapsed();
        
        // ✅ FLIGHT RECORDER: Registrar fin de price fetch
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "price_fetch_internal", start_time, serde_json::json!({
                "prices_fetched": results.len(),
                "tokens_requested": tokens.len(),
                "chainlink_prices": chainlink_prices.len(),
                "pool_fallback_prices": pool_fallback_prices.len(),
                "duration_ms": duration.as_millis()
            }));
        }
        
        Ok(results)
    }

    async fn fetch_from_chainlink(&self, tokens: &[Address], block: Option<BlockId>) -> Result<HashMap<Address, f64>> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de Chainlink fetch
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "chainlink_fetch", serde_json::json!({
                "tokens_count": tokens.len()
            }));
        }
        
        let mut prices = HashMap::new();
        // ✅ OPTIMIZACIÓN LATENCIA: Timeout aumentado a 300ms para mejorar success rate
        // Si Chainlink no responde en 300ms, mejor usar cache del bloque anterior
        // Los precios de Chainlink no cambian significativamente entre bloques (250ms)
        // ✅ AJUSTE: 1 retry rápido para reducir fallos transitorios sin afectar latencia mucho
        let multicall = Multicall::new(self.provider.clone(), self.multicall_address, self.multicall_batch_size)
            .with_timeout(1) // 1 segundo máximo (pero esperamos <300ms)
            .with_retries(1); // 1 retry rápido para fallos transitorios
        let dummy = AggregatorV3Interface::new(Address::zero(), self.provider.clone());
        let latest_fn = dummy.abi().function("latestRoundData")?;
        let decimals_fn = dummy.abi().function("decimals")?;

        let mut calls = Vec::new();
        let mut oracle_map = Vec::new();
        let mut decimals_calls: Vec<Call> = Vec::new();
        let mut decimals_oracles: Vec<Address> = Vec::new();

        log::info!("🔍 fetch_from_chainlink: {} tokens requested, {} oracles configured", tokens.len(), self.oracle_addresses.len());
        for &token in tokens {
            if let Some(&oracle) = self.oracle_addresses.get(&token) {
                // Prefetch decimals del oracle una sola vez (cache en memoria)
                if self.oracle_decimals_cache.get(&oracle).is_none() {
                    let aggr = AggregatorV3Interface::new(oracle, self.provider.clone());
                    if let Some(calldata) = aggr.decimals().calldata() {
                        decimals_calls.push(Call { target: oracle, call_data: calldata });
                        decimals_oracles.push(oracle);
                    }
                }
                let aggr = AggregatorV3Interface::new(oracle, self.provider.clone());
                calls.push(Call { target: oracle, call_data: aggr.latest_round_data().calldata().unwrap() });
                oracle_map.push((token, oracle));
                log::info!("  ✅ Found oracle for token {:?}: {:?}", token, oracle);
            } else {
                log::warn!("  ⚠️ No oracle configured for token {:?}", token);
            }
        }

        // 1) Resolver decimals (solo para oracles sin cache)
        if !decimals_calls.is_empty() {
            log::info!("🔢 Fetching decimals for {} Chainlink oracles (cache miss)...", decimals_calls.len());
            match multicall.run(decimals_calls, block).await {
                Ok(results) => {
                    for (i, bytes) in results.into_iter().enumerate() {
                        let oracle = decimals_oracles[i];
                        if bytes.is_empty() {
                            continue;
                        }
                        if let Ok(decoded) = decimals_fn.decode_output(&bytes) {
                            if let Some(d) = decoded.get(0).and_then(|t| t.clone().into_uint()) {
                                let dec_u32 = d.as_u32();
                                // Chainlink feeds típicamente 8 o 18; clamp defensivo
                                let dec_u8 = if dec_u32 <= 36 { dec_u32 as u8 } else { 8u8 };
                                self.oracle_decimals_cache.insert(oracle, dec_u8);
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("⚠️ Failed to fetch oracle decimals via multicall: {:?} (fallback to 8)", e);
                }
            }
        }

        if calls.is_empty() {
            log::warn!("⚠️ No Chainlink calls to make (no oracles configured for requested tokens)");
            return Ok(prices);
        }

        log::info!("📞 Making {} Chainlink multicall requests...", calls.len());
        let fetch_start = std::time::Instant::now();
        log::info!("  ⏱️ Starting multicall at {:?}...", fetch_start);
        let call_results = match multicall.run(calls, block).await {
            Ok(results) => {
                let fetch_duration = fetch_start.elapsed();
                log::info!("  ✅ Multicall completed in {:?}, got {} responses", fetch_duration, results.len());
                results
            }
            Err(e) => {
                let fetch_duration = fetch_start.elapsed();
                log::warn!("  ⚠️ Multicall failed after {:?}: {:?}", fetch_duration, e);
                return Err(e);
            }
        };

        log::info!("  📊 Multicall returned {} results, processing...", call_results.len());
        if call_results.is_empty() {
            log::warn!("  ⚠️ Multicall returned empty results - no prices to process");
            return Ok(prices);
        }

        for (i, bytes) in call_results.into_iter().enumerate() {
            let (token, oracle) = oracle_map[i];
            if bytes.is_empty() { 
                log::warn!("  ⚠️ Empty response for token {:?} from oracle {:?}", token, oracle);
                continue; 
            }
            log::info!("  📦 Processing response {}: {} bytes for token {:?}", i, bytes.len(), token);
            if let Ok(decoded) = latest_fn.decode_output(&bytes) {
                log::info!("  ✅ Successfully decoded output for token {:?}", token);
                if let Some(answer) = chainlink_answer_token_to_u256(decoded.get(1)) {
                    let decimals = self.oracle_decimals_cache
                        .get(&oracle)
                        .map(|e| *e.value())
                        .unwrap_or(8u8);
                    let price = u256_div_10_pow(answer, decimals as u32);
                    if price > 0.0 {
                        prices.insert(token, price);
                        log::info!("  ✅ Got price for token {:?}: ${:.2}", token, price);
                    } else {
                        log::warn!("  ⚠️ Invalid price (0) for token {:?} from oracle {:?}", token, oracle);
                    }
                } else {
                    log::warn!(
                        "  ⚠️ Failed to extract answer (decoded[1] as int256) for token {:?} from oracle {:?}",
                        token,
                        oracle
                    );
                }
            } else {
                log::warn!("  ⚠️ Failed to decode output for token {:?} from oracle {:?} (bytes len: {})", token, oracle, bytes.len());
            }
        }
        log::info!("✅ fetch_from_chainlink: got {} prices out of {} requested", prices.len(), tokens.len());
        
        // ✅ FLIGHT RECORDER: Registrar fin de Chainlink fetch
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "chainlink_fetch", start_time, serde_json::json!({
                "prices_fetched": prices.len(),
                "tokens_requested": tokens.len()
            }));
        }
        
        Ok(prices)
    }

    /// Public method to fetch prices from pool fallback (for background updater)
    pub async fn fetch_from_twap_fallback(&self, tokens: &[Address], block: Option<BlockId>, known_prices: &HashMap<Address, f64>) -> Result<HashMap<Address, f64>> {
        let start_time = Instant::now();
        
        // ✅ FLIGHT RECORDER: Registrar inicio de pool fallback
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(recorder, "pool_fallback", serde_json::json!({
                "tokens_count": tokens.len(),
                "known_prices_count": known_prices.len()
            }));
        }
        
        // ✅ AJUSTE 3: Pool fallback optimizado con batch get_pool (reduce RPC calls significativamente)
        // Estrategia: Batch todas las llamadas get_pool, luego usar método individual para observe solo de pools existentes
        let mut prices = HashMap::new();
        
        if tokens.is_empty() || known_prices.is_empty() {
            return Ok(prices);
        }
        
        let block_id = block.unwrap_or(BlockId::Number(BlockNumber::Latest));
        // ✅ FASE 3.2: Usar batch size más pequeño para pool fallback (20 en vez de 50-100)
        const POOL_FALLBACK_BATCH_SIZE: usize = 20;
        // ✅ FIX: Timeout de 2s para pool fallback (suficiente para multicalls grandes)
        let multicall = Multicall::new(self.provider.clone(), self.multicall_address, POOL_FALLBACK_BATCH_SIZE)
            .with_timeout(2) // 2 segundos (suficiente para multicalls grandes)
            .with_retries(0); // Sin retries para pool fallback (no crítico)
        
        // Paso 1: Preparar todas las llamadas get_pool en un multicall
        let mut get_pool_calls = Vec::new();
        let mut call_map: Vec<(Address, Address, u32)> = Vec::new(); // (token, anchor, fee)
        
        for &token in tokens {
            // Si ya tenemos precio para este token, skip
            if prices.contains_key(&token) { continue; }
            
            for &anchor in &self.anchor_tokens {
                if token == anchor { continue; }
                if !known_prices.contains_key(&anchor) { continue; }
                
                for fee in &[500, 3000, 100] {
                    let get_pool_call = self.uniswap_v3_factory.get_pool(token, anchor, *fee);
                    if let Some(calldata) = get_pool_call.calldata() {
                        get_pool_calls.push(Call {
                            target: self.uniswap_v3_factory.address(),
                            call_data: calldata,
                        });
                        call_map.push((token, anchor, *fee));
                    }
                }
            }
        }
        
        if get_pool_calls.is_empty() {
            return Ok(prices);
        }
        
        log::info!("  🔄 Pool fallback: Batch fetching {} pool addresses...", get_pool_calls.len());
        let get_pool_start = std::time::Instant::now();
        
        // Paso 2: Ejecutar multicall para get_pool
        let get_pool_results = match multicall.run(get_pool_calls, block).await {
            Ok(results) => results,
            Err(e) => {
                log::debug!("  ⚠️ Pool fallback get_pool multicall failed: {:?} (non-critical)", e);
                return Ok(prices);
            }
        };
        
        // Paso 3: Identificar pools existentes y preparar batch de observe + token_0
        // ✅ OPTIMIZACIÓN: Batch de observe y token_0 en lugar de llamadas individuales
        let mut pool_data_map: Vec<(Address, Address, Address, f64)> = Vec::new(); // (pool_address, token, anchor, anchor_price)
        
        for (i, result) in get_pool_results.iter().enumerate() {
            if result.is_empty() { continue; }
            
            let (token, anchor, _fee) = call_map[i];
            
            // Si ya procesamos este token exitosamente, skip
            if prices.contains_key(&token) { continue; }
            
            // Decodificar pool address (primeros 32 bytes, skip padding de 12 bytes)
            if result.len() >= 32 {
                let pool_address = Address::from_slice(&result[12..32]);
                
                if pool_address != Address::zero() {
                    if let Some(&anchor_price) = known_prices.get(&anchor) {
                        pool_data_map.push((pool_address, token, anchor, anchor_price));
                    }
                }
            }
        }
        
        if pool_data_map.is_empty() {
            log::info!("  ✅ Pool fallback: No valid pools found (get_pool took {:?})", get_pool_start.elapsed());
            return Ok(prices);
        }
        
        // ✅ FASE 3.2: Limitar número de pools consultados en pool fallback
        const MAX_POOLS_FOR_FALLBACK: usize = 50; // Reducir de potencialmente 195+ a 50
        let pools_to_query: Vec<_> = pool_data_map.iter()
            .take(MAX_POOLS_FOR_FALLBACK)
            .collect();
        
        if pool_data_map.len() > MAX_POOLS_FOR_FALLBACK {
            log::info!("  🔄 Pool fallback: Limiting to {} pools (from {} available)", 
                      MAX_POOLS_FOR_FALLBACK, pool_data_map.len());
        } else {
            log::info!("  🔄 Pool fallback: Found {} valid pools, preparing batch observe+token_0 calls...", pool_data_map.len());
        }
        
        // Paso 4: Batch de observe y token_0 calls
        let mut observe_calls = Vec::new();
        let mut token0_calls = Vec::new();
        let mut pool_index_map: Vec<usize> = Vec::new(); // Map from call index to pool_data_map index
        
        // ✅ FASE 3.2: Usar POOL_FALLBACK_BATCH_SIZE ya definido arriba (línea 720)
        for (pool_idx, (pool_address, _token, _anchor, _anchor_price)) in pools_to_query.iter().enumerate() {
            let pool_contract = crate::contracts::uniswap_v3::UniswapV3Pool::new(*pool_address, self.provider.clone());
            
            // observe call
            if let Some(observe_calldata) = pool_contract.observe(vec![60, 0]).calldata() {
                observe_calls.push(Call {
                    target: *pool_address,
                    call_data: observe_calldata,
                });
                pool_index_map.push(pool_idx);
            }
            
            // token_0 call
            if let Some(token0_calldata) = pool_contract.token_0().calldata() {
                token0_calls.push(Call {
                    target: *pool_address,
                    call_data: token0_calldata,
                });
            }
        }
        
        // Ejecutar batch de observe
        let observe_start = std::time::Instant::now();
        let observe_results = if !observe_calls.is_empty() {
            log::info!("  🔄 Pool fallback: Executing batch observe multicall ({} calls)...", observe_calls.len());
            match multicall.run(observe_calls, block).await {
                Ok(results) => results,
                Err(e) => {
                    log::debug!("  ⚠️ Pool fallback observe multicall failed: {:?} (non-critical)", e);
                    return Ok(prices);
                }
            }
        } else {
            Vec::new()
        };
        
        // Ejecutar batch de token_0
        let token0_start = std::time::Instant::now();
        let token0_results = if !token0_calls.is_empty() {
            log::info!("  🔄 Pool fallback: Executing batch token_0 multicall ({} calls, observe took {:?})...", token0_calls.len(), observe_start.elapsed());
            match multicall.run(token0_calls, block).await {
                Ok(results) => results,
                Err(e) => {
                    log::debug!("  ⚠️ Pool fallback token_0 multicall failed: {:?} (non-critical)", e);
                    return Ok(prices);
                }
            }
        } else {
            Vec::new()
        };
        
        // Paso 5: Procesar resultados y calcular precios
        let mut processed_tokens = std::collections::HashSet::new();
        
        for (observe_idx, observe_result) in observe_results.iter().enumerate() {
            if observe_result.is_empty() { continue; }
            
            let pool_idx = pool_index_map[observe_idx];
            let (pool_address, token, anchor, anchor_price) = &pool_data_map[pool_idx];
            
            // Si ya procesamos este token exitosamente, skip
            if processed_tokens.contains(token) { continue; }
            
            // Decodificar observe result: (int56[] tickCumulatives, uint160[] secondsPerLiquidityCumulativeX128s)
            // Necesitamos decodificar manualmente porque el resultado es una tupla
            if let Ok(decoded) = ethers::abi::decode(
                &[
                    ethers::abi::ParamType::Array(Box::new(ethers::abi::ParamType::Int(56))),
                    ethers::abi::ParamType::Array(Box::new(ethers::abi::ParamType::Uint(160))),
                ],
                observe_result
            ) {
                if let (Some(ethers::abi::Token::Array(tick_cumulatives)), _) = (decoded.get(0), decoded.get(1)) {
                    if tick_cumulatives.len() >= 2 {
                        let tick_cum_1 = if let Some(ethers::abi::Token::Int(t1)) = tick_cumulatives.get(1) {
                            I256::from_raw(*t1)
                        } else {
                            continue;
                        };
                        let tick_cum_0 = if let Some(ethers::abi::Token::Int(t0)) = tick_cumulatives.get(0) {
                            I256::from_raw(*t0)
                        } else {
                            continue;
                        };
                        
                        let avg_tick = calculate_twap_tick(tick_cum_1, tick_cum_0, 60);
                        
                        // Obtener token0 del resultado batch
                        let token0 = if observe_idx < token0_results.len() && !token0_results[observe_idx].is_empty() {
                            // Decodificar token0 (address = 32 bytes, skip padding de 12 bytes)
                            if token0_results[observe_idx].len() >= 32 {
                                Address::from_slice(&token0_results[observe_idx][12..32])
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        };
                        
                        // Calcular relative_price desde el tick (la función tick_to_price ahora maneja overflow)
                        let relative_price = crate::v3_math::tick_to_price(avg_tick);
                        
                        // ✅ VALIDACIÓN 1: Verificar que relative_price sea válido (no inf, nan, o 0.0)
                        if !relative_price.is_finite() || relative_price <= 0.0 {
                            log::debug!("  ⚠️ Pool fallback: Rejected invalid relative_price {} (tick: {}) for token {:?} (pool {:?})", 
                                      relative_price, avg_tick, token, pool_address);
                            continue;
                        }
                        
                        // Calcular precio final en USD
                        let price = if token0 == *token {
                            relative_price * anchor_price
                        } else {
                            // Verificar que relative_price no sea muy pequeño para evitar overflow en división
                            if relative_price < 1e-20 {
                                log::debug!("  ⚠️ Pool fallback: Rejected very small relative_price {} (tick: {}) for token {:?} (pool {:?}) - would cause overflow", 
                                          relative_price, avg_tick, token, pool_address);
                                continue;
                            }
                            (1.0 / relative_price) * anchor_price
                        };
                        
                        // ✅ VALIDACIÓN 2: Verificar que el precio calculado sea válido (no inf, nan)
                        if !price.is_finite() || price <= 0.0 {
                            log::debug!("  ⚠️ Pool fallback: Rejected invalid calculated price {} (tick: {}, relative_price: {:.6}) for token {:?} (pool {:?})", 
                                      price, avg_tick, relative_price, token, pool_address);
                            continue;
                        }
                        
                        // ✅ VALIDACIÓN 3: Validación de precio USD final (rango humano razonable)
                        // Precio debe estar entre $0.00000001 y $10,000,000 USD
                        // Este rango cubre desde micro-tokens hasta tokens muy valiosos
                        const MIN_USD_PRICE: f64 = 0.00000001; // $0.00000001 (1e-8)
                        const MAX_USD_PRICE: f64 = 10_000_000.0; // $10M USD
                        
                        if price >= MIN_USD_PRICE && price <= MAX_USD_PRICE {
                            prices.insert(*token, price);
                            processed_tokens.insert(*token);
                            log::info!("  ✅ Pool fallback: Accepted price for token {:?} from pool {:?}: ${:.8} (tick: {}, relative_price: {:.6}, anchor: ${:.2})", 
                                      token, pool_address, price, avg_tick, relative_price, anchor_price);
                        } else {
                            log::warn!("  ⚠️ Pool fallback: Rejected price outside USD range for token {:?}: ${:.8} (tick: {}, relative_price: {:.6}, anchor: ${:.2}, range: ${:.8}-${:.2})", 
                                      token, price, avg_tick, relative_price, anchor_price, MIN_USD_PRICE, MAX_USD_PRICE);
                        }
                    }
                }
            }
        }
        
        let total_duration = get_pool_start.elapsed();
        log::info!("  ✅ Pool fallback: Calculated {} prices from pools in {:?} (get_pool: {:?}, observe: {:?}, token_0: {:?})", 
                   prices.len(), total_duration, get_pool_start.elapsed(), observe_start.elapsed(), token0_start.elapsed());
        
        // ✅ FLIGHT RECORDER: Registrar fin de pool fallback
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_end!(recorder, "pool_fallback", start_time, serde_json::json!({
                "prices_fetched": prices.len(),
                "tokens_requested": tokens.len(),
                "duration_ms": total_duration.as_millis()
            }));
        }
        
        Ok(prices)
    }
    
    // Helper method para calcular precio desde un pool específico (usado por batch fallback)
    async fn get_price_via_fallback_single_pool(
        &self,
        token: Address,
        anchor: Address,
        pool_address: Address,
        anchor_price: f64,
        block: Option<BlockId>,
    ) -> Result<Option<f64>> {
        let pool = crate::contracts::uniswap_v3::UniswapV3Pool::new(pool_address, self.provider.clone());
        let block_id = block.unwrap_or(BlockId::Number(BlockNumber::Latest));
        
        if let Ok((tick_cumulatives, _)) = pool.observe(vec![60, 0]).call_raw().block(block_id).await {
            if tick_cumulatives.len() == 2 {
                let avg_tick = calculate_twap_tick(tick_cumulatives[1].into(), tick_cumulatives[0].into(), 60);
                
                // Calcular relative_price desde el tick (la función tick_to_price ahora maneja overflow)
                let relative_price = crate::v3_math::tick_to_price(avg_tick);
                
                // ✅ VALIDACIÓN 1: Verificar que relative_price sea válido (no inf, nan, o 0.0)
                if !relative_price.is_finite() || relative_price <= 0.0 {
                    log::debug!("  ⚠️ Pool fallback (single): Rejected invalid relative_price {} (tick: {}) for token {:?}", 
                              relative_price, avg_tick, token);
                    return Ok(None);
                }
                
                let token0 = pool.token_0().call().await?;
                
                // Calcular precio final en USD
                let price = if token0 == token {
                    relative_price * anchor_price
                } else {
                    // Verificar que relative_price no sea muy pequeño para evitar overflow en división
                    if relative_price < 1e-20 {
                        log::debug!("  ⚠️ Single pool fallback: Rejected very small relative_price {} (tick: {}) for token {:?} - would cause overflow", 
                                  relative_price, avg_tick, token);
                        return Ok(None);
                    }
                    (1.0 / relative_price) * anchor_price
                };
                
                // ✅ VALIDACIÓN 2: Verificar que el precio calculado sea válido (no inf, nan)
                if !price.is_finite() || price <= 0.0 {
                    log::debug!("  ⚠️ Single pool fallback: Rejected invalid calculated price {} (tick: {}, relative_price: {:.6}) for token {:?}", 
                              price, avg_tick, relative_price, token);
                    return Ok(None);
                }
                
                // ✅ VALIDACIÓN 3: Validación de precio USD final (rango humano razonable)
                const MIN_USD_PRICE: f64 = 0.00000001; // $0.00000001 (1e-8)
                const MAX_USD_PRICE: f64 = 10_000_000.0; // $10M USD
                
                if price >= MIN_USD_PRICE && price <= MAX_USD_PRICE {
                    log::info!("  ✅ Pool fallback (single): Accepted price for token {:?} from pool {:?}: ${:.8} (tick: {}, relative_price: {:.6}, anchor: ${:.2})", 
                              token, pool_address, price, avg_tick, relative_price, anchor_price);
                    return Ok(Some(price));
                } else {
                    log::warn!("  ⚠️ Single pool fallback: Rejected price outside USD range for token {:?}: ${:.8} (tick: {}, relative_price: {:.6}, anchor: ${:.2}, range: ${:.8}-${:.2})", 
                              token, price, avg_tick, relative_price, anchor_price, MIN_USD_PRICE, MAX_USD_PRICE);
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    async fn get_price_via_fallback(&self, token: Address, block: Option<BlockId>, known_prices: &HashMap<Address, f64>) -> Result<Option<f64>> {
        for &anchor in &self.anchor_tokens {
            if token == anchor { continue; }
            if let Some(anchor_price) = known_prices.get(&anchor) {
                for fee in &[500, 3000, 100] { // Common fee tiers
                    let pool_address: Address = self.uniswap_v3_factory.get_pool(token, anchor, *fee).call_raw().block(block.unwrap_or(BlockId::Number(BlockNumber::Latest))).await?.into();

                    if pool_address != Address::zero() {
                        let pool = crate::contracts::uniswap_v3::UniswapV3Pool::new(pool_address, self.provider.clone());
                        if let Ok((tick_cumulatives, _)) = pool.observe(vec![60, 0]).call_raw().block(block.unwrap_or(BlockId::Number(BlockNumber::Latest))).await {
                            if tick_cumulatives.len() == 2 {
                                let avg_tick = calculate_twap_tick(tick_cumulatives[1].into(), tick_cumulatives[0].into(), 60);
                                let token0 = pool.token_0().call().await?;
                                let relative_price = crate::v3_math::tick_to_price(avg_tick);
                                let price = if token0 == token {
                                    relative_price * anchor_price
                                } else {
                                    (1.0/relative_price) * anchor_price
                                };
                                return Ok(Some(price));
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    /// Fetches token decimals using multicall for efficiency.
    pub async fn fetch_token_decimals(&self, tokens: &[Address], block: Option<BlockId>) -> Result<()> {
        // FASE 4.1: Lock-free access
        let decimals_cache = &self.token_decimals_cache;
        let tokens_to_fetch: Vec<Address> = tokens.iter().filter(|t| !decimals_cache.contains_key(t)).cloned().collect();

        if tokens_to_fetch.is_empty() {
            return Ok(());
        }

        info!("Fetching decimals for {} tokens", tokens_to_fetch.len());

        let multicall = Multicall::new(self.provider.clone(), self.multicall_address, self.multicall_batch_size);
        let erc20_dummy = ERC20Minimal::new(Address::zero(), self.provider.clone());
        let decimals_fn = erc20_dummy.abi().function("decimals")?;

        let calls: Vec<Call> = tokens_to_fetch.iter().map(|&token| {
            let c = ERC20Minimal::new(token, self.provider.clone());
            Call {
                target: token,
                call_data: c.decimals().calldata().unwrap(),
            }
        }).collect();

        let results = multicall.run(calls, block).await?;

        for (i, bytes) in results.into_iter().enumerate() {
            let token = tokens_to_fetch[i];
            if !bytes.is_empty() {
                if let Ok(decoded) = decimals_fn.decode_output(&bytes) {
                    if let Some(dec) = decoded[0].clone().into_uint() {
                        match TryInto::<u32>::try_into(dec) {
                            Ok(d) if d <= 255 => {
                                decimals_cache.insert(token, d as u8);
                            }
                            _ => {
                                warn!("Decimal value for {:?} is out of u8 range (value: {}), defaulting to 18.", token, dec);
                                decimals_cache.insert(token, 18);
                            }
                        }
                    }
                }
            } else {
                // Default to 18 if call fails
                warn!("Failed to fetch decimals for {:?}, defaulting to 18.", token);
                decimals_cache.insert(token, 18);
            }
        }
        Ok(())
    }

    /// Convert amount to USD value, normalizing by token decimals.
    pub async fn get_amount_usd(&self, amount: U256, token: Address, block: Option<BlockId>) -> Result<f64> {
        let price = self.get_usd_price_at(token, block).await?;

        // FASE 4.1: Lock-free access
        let decimals = self.token_decimals_cache.get(&token).map(|e| *e.value()).unwrap_or(18);

        // Convert U256 to f64 safely to avoid overflow
        let amount_f64 = u256_div_10_pow(amount, decimals as u32);

        Ok(amount_f64 * price)
    }

    /// Convert gas cost (in WEI) to USD value.
    pub async fn get_gas_cost_usd(&self, gas_cost_wei: U256, weth_address: Address, block: Option<BlockId>) -> Result<f64> {
        let eth_price = self.get_usd_price_at(weth_address, block).await?;
        
        // Convert U256 to f64 safely for gas cost
        let gas_cost_eth = u256_div_10_pow(gas_cost_wei, 18);
        
        Ok(gas_cost_eth * eth_price)
    }
}

// Safely divide a U256 by 10^decimals and return f64 without intermediate u128 casts.
fn u256_div_10_pow(value: U256, decimals: u32) -> f64 {
    if value.is_zero() { return 0.0; }
    let s = value.to_string();
    let len = s.len();
    let d = decimals as usize;
    let dec_str = if d == 0 {
        s
    } else if len <= d {
        let mut out = String::with_capacity(2 + d);
        out.push_str("0.");
        if d > len { out.push_str(&"0".repeat(d - len)); }
        out.push_str(&s);
        out
    } else {
        let int_part = &s[..len - d];
        let frac_part = &s[len - d..];
        let mut out = String::with_capacity(len + 1);
        out.push_str(int_part);
        out.push('.');
        out.push_str(frac_part);
        out
    };
    dec_str.parse::<f64>().unwrap_or(0.0)
}

#[cfg(test)]
mod chainlink_decimal_scaling_tests {
    use super::u256_div_10_pow;
    use ethers::types::U256;

    #[test]
    fn test_chainlink_price_scaling_8_decimals() {
        // 3000.00 * 1e8
        let answer = U256::from(3000u64) * U256::from(100_000_000u64);
        let p = u256_div_10_pow(answer, 8);
        assert!((p - 3000.0).abs() < 1e-6, "price={}", p);
    }

    #[test]
    fn test_chainlink_price_scaling_18_decimals() {
        // 3000.00 * 1e18
        let answer = U256::from(3000u64) * U256::exp10(18);
        let p = u256_div_10_pow(answer, 18);
        assert!((p - 3000.0).abs() < 1e-6, "price={}", p);
    }
}

/// Calculates the time-weighted average tick from two cumulative tick values.
fn calculate_twap_tick(tick_cumulative_end: I256, tick_cumulative_start: I256, time_delta: u32) -> i64 {
    if time_delta == 0 {
        return 0; // Avoid division by zero
    }
    let tick_diff = tick_cumulative_end - tick_cumulative_start;
    // Use as_i128 to handle potential negative results correctly
    (tick_diff.as_i128() / time_delta as i128) as i64
}

/// Parse Chainlink `latestRoundData().answer` token into a positive `U256`.
///
/// Chainlink returns `int256 answer`. ethers represents `int256` ABI token as a raw `U256` (two's complement).
/// Returns `Some(U256)` only for strictly-positive answers; returns `None` for missing token, non-int token,
/// zero, or negative values.
fn chainlink_answer_token_to_u256(t: Option<&Token>) -> Option<U256> {
    let raw = t?.clone().into_int()?;
    let signed = I256::from_raw(raw);
    if signed <= I256::zero() {
        return None;
    }
    Some(signed.into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chainlink_answer_parsing_accepts_positive_and_rejects_non_positive() {
        // Positive answer: token stores int256 as raw U256.
        let t_pos = Token::Int(U256::from(1234u64));
        assert_eq!(chainlink_answer_token_to_u256(Some(&t_pos)), Some(U256::from(1234u64)));

        // Zero should be rejected.
        let t_zero = Token::Int(U256::zero());
        assert_eq!(chainlink_answer_token_to_u256(Some(&t_zero)), None);

        // Negative answer should be rejected (two's complement raw value).
        let neg_raw = I256::from(-5i64).into_raw();
        let t_neg = Token::Int(neg_raw);
        assert_eq!(chainlink_answer_token_to_u256(Some(&t_neg)), None);
    }
}