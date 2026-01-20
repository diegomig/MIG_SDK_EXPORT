// routegen-rs/src/background_price_updater.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use tokio::time::{interval, Duration, Instant};
use ethers::prelude::*;
use dashmap::DashMap;
use std::collections::HashMap;
use log::{info, warn, error};
use anyhow::Result;

/// Cache compartido con metadata de freshness
#[derive(Clone)]
pub struct SharedPriceCache {
    prices: Arc<DashMap<Address, PriceEntry>>,
    last_successful_update: Arc<AtomicU64>,
    consecutive_failures: Arc<AtomicU32>,
}

#[derive(Debug, Clone)]
pub struct PriceEntry {
    pub price: f64,
    pub updated_at: Instant,
    pub source: PriceSource,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceSource {
    Chainlink,
    PoolBased,
    Hardcoded,
    Stale, // Precio viejo pero mejor que nada
}

impl SharedPriceCache {
    pub fn new() -> Self {
        Self {
            prices: Arc::new(DashMap::new()),
            last_successful_update: Arc::new(AtomicU64::new(0)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
        }
    }
    
    /// Lee precio con metadata de freshness
    pub fn get_price_with_metadata(&self, token: &Address) -> Option<(f64, Duration, PriceSource)> {
        self.prices.get(token).map(|entry| {
            let age = entry.updated_at.elapsed();
            (entry.price, age, entry.source)
        })
    }
    
    /// Lee precio simple (para hot path)
    pub fn get_price(&self, token: &Address) -> Option<f64> {
        self.prices.get(token).map(|entry| entry.price)
    }
    
    /// Obtiene múltiples precios con información de freshness
    pub fn get_prices_batch(&self, tokens: &[Address]) -> (HashMap<Address, f64>, CacheStats) {
        let mut result = HashMap::new();
        let mut stats = CacheStats::default();
        
        for token in tokens {
            if let Some((price, age, source)) = self.get_price_with_metadata(token) {
                result.insert(*token, price);
                
                // Clasificar por freshness
                if age < Duration::from_secs(5) {
                    stats.fresh += 1;
                } else if age < Duration::from_secs(30) {
                    stats.acceptable += 1;
                } else {
                    stats.stale += 1;
                }
                
                // Clasificar por source
                match source {
                    PriceSource::Chainlink => stats.from_chainlink += 1,
                    PriceSource::PoolBased => stats.from_pools += 1,
                    PriceSource::Hardcoded => stats.from_hardcoded += 1,
                    PriceSource::Stale => stats.from_stale += 1,
                }
            } else {
                stats.missing += 1;
            }
        }
        
        (result, stats)
    }
    
    pub fn update_batch(&self, prices: HashMap<Address, f64>, source: PriceSource) {
        let now = Instant::now();
        for (token, price) in prices {
            self.prices.insert(token, PriceEntry {
                price,
                updated_at: now,
                source,
            });
        }
    }
    
    /// ✅ SPRINT ESTABILIZACIÓN: Set precio individual (para emergency fetch)
    pub fn set_price(&self, token: Address, price: f64, block: u64) {
        self.prices.insert(token, PriceEntry {
            price,
            updated_at: Instant::now(),
            source: PriceSource::Chainlink, // Asumir Chainlink para emergency fetch
        });
        // Actualizar last_successful_update si es necesario
        self.last_successful_update.store(block, Ordering::Relaxed);
    }
    
    pub fn is_healthy(&self) -> bool {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);
        let last_update = self.last_successful_update.load(Ordering::Relaxed);
        
        // Si nunca hubo actualización exitosa, no está healthy
        if last_update == 0 {
            return failures < 3; // Si tiene <3 fallos, aún puede estar inicializando
        }
        
        // Calcular edad de última actualización
        // last_update es un timestamp en segundos desde algún epoch
        // Por simplicidad, asumimos que si last_update > 0, fue actualizado recientemente
        // En producción, deberíamos usar SystemTime o similar
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let age = now.saturating_sub(last_update);
        
        // Healthy si: <3 fallos consecutivos Y última actualización <60s
        failures < 3 && age < 60
    }
    
    pub fn mark_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_successful_update.store(now, Ordering::Relaxed);
    }
    
    pub fn mark_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Default, Debug)]
pub struct CacheStats {
    pub fresh: usize,      // <5s
    pub acceptable: usize, // 5-30s
    pub stale: usize,      // >30s
    pub missing: usize,
    pub from_chainlink: usize,
    pub from_pools: usize,
    pub from_hardcoded: usize,
    pub from_stale: usize,
}

/// Background updater con fallbacks
pub struct BackgroundPriceUpdater<M: Middleware> {
    cache: SharedPriceCache,
    chainlink_feed: Arc<crate::price_feeds::PriceFeed<M>>,
    tokens_to_track: Arc<tokio::sync::RwLock<Vec<Address>>>,
    update_interval: Duration,
}

impl<M: Middleware> BackgroundPriceUpdater<M> where M: 'static {
    pub fn new(
        cache: SharedPriceCache,
        chainlink_feed: Arc<crate::price_feeds::PriceFeed<M>>,
        tokens_to_track: Vec<Address>,
        update_interval_secs: u64,
    ) -> Self {
        Self {
            cache,
            chainlink_feed,
            tokens_to_track: Arc::new(tokio::sync::RwLock::new(tokens_to_track)),
            update_interval: Duration::from_secs(update_interval_secs),
        }
    }

    /// Replace the tracked token list (background-safe). Keeps list unique/sorted.
    pub async fn set_tokens_to_track(&self, mut tokens: Vec<Address>) {
        tokens.retain(|t| *t != Address::zero());
        tokens.sort();
        tokens.dedup();
        let mut guard = self.tokens_to_track.write().await;
        *guard = tokens;
    }

    /// Add tokens to the tracked list (background-safe). Returns how many were newly added.
    pub async fn add_tokens_to_track(&self, tokens: &[Address]) -> usize {
        let mut guard = self.tokens_to_track.write().await;
        let before = guard.len();
        for &t in tokens {
            if t != Address::zero() {
                guard.push(t);
            }
        }
        guard.sort();
        guard.dedup();
        guard.len().saturating_sub(before)
    }
    
    pub async fn start(self: Arc<Self>) {
        info!("🚀 Starting background price updater");
        let token_count = self.tokens_to_track.read().await.len();
        info!("   Tokens: {}", token_count);
        info!("   Interval: {:?}", self.update_interval);
        
        // Warm-up inicial
        info!("🔥 Initial warm-up...");
        match self.update_prices_with_fallback().await {
            Ok(stats) => {
                info!("✅ Warm-up successful: {}", stats);
                self.cache.mark_success();
            }
            Err(e) => {
                error!("❌ Warm-up failed: {}", e);
                warn!("⚠️ Starting with empty cache - first blocks may have no prices");
                self.cache.mark_failure();
            }
        }
        
        let mut ticker = interval(self.update_interval);
        let mut iteration = 0u64;
        
        loop {
            ticker.tick().await;
            iteration += 1;
            
            match self.update_prices_with_fallback().await {
                Ok(stats) => {
                    info!("✅ [Background #{}] {}", iteration, stats);
                    self.cache.mark_success();
                }
                Err(e) => {
                    self.cache.mark_failure();
                    let failures = self.cache.consecutive_failures.load(Ordering::Relaxed);
                    
                    error!(
                        "❌ [Background #{}] Update failed (consecutive failures: {}): {}",
                        iteration,
                        failures,
                        e
                    );
                    
                    if failures >= 3 {
                        error!("🚨 Background updater is unhealthy! Cache may be stale.");
                    }
                }
            }
        }
    }
    
    async fn update_prices_with_fallback(&self) -> Result<UpdateStats> {
        let start = Instant::now();
        let mut stats = UpdateStats::default();
        let tokens_to_track: Vec<Address> = self.tokens_to_track.read().await.clone();
        if tokens_to_track.is_empty() {
            return Ok(stats);
        }
        
        // ✅ OPTIMIZACIÓN: Consultar SharedPriceCache PRIMERO para evitar llamadas innecesarias
        let mut prices = HashMap::new();
        let mut tokens_still_needed = Vec::new();
        let mut cache_hits = 0;
        
        for &token in &tokens_to_track {
            if let Some(price) = self.cache.get_price(&token) {
                if price > 0.0 {
                    prices.insert(token, price);
                    cache_hits += 1;
                } else {
                    tokens_still_needed.push(token);
                }
            } else {
                tokens_still_needed.push(token);
            }
        }
        
        if cache_hits > 0 {
            info!("  📊 SharedPriceCache provided {} prices (skipping fetch for these tokens)", cache_hits);
        }
        
        // 1. Intentar Chainlink (primario) con timeout razonable solo para tokens que faltan
        if tokens_still_needed.is_empty() {
            // Todos los precios ya están en cache, no necesitamos hacer llamadas
            stats.chainlink_success = true;
            stats.chainlink_count = 0;
        } else {
            let chainlink_result = tokio::time::timeout(
                Duration::from_millis(2000), // Timeout de 2s para background (Chainlink puede tardar ~170ms)
                self.chainlink_feed
                    // Background can wait longer without increasing RPC/CU, and avoids "missing=1" forever.
                    // ✅ Pasar SharedPriceCache para que pueda usar anchor tokens y ejecutar pool fallback en paralelo
                    .get_usd_prices_batch_with_chainlink_timeout_and_cache(&tokens_still_needed, None, Duration::from_millis(1500), Some(&self.cache))
            ).await;
        
            match chainlink_result {
                Ok(Ok(chainlink_prices)) => {
                    stats.chainlink_success = true;
                    stats.chainlink_count = chainlink_prices.len();
                    prices.extend(chainlink_prices);
                }
                Ok(Err(e)) => {
                    warn!("⚠️ Chainlink failed: {}", e);
                    stats.chainlink_success = false;
                }
                Err(_) => {
                    warn!("⚠️ Chainlink timeout (2000ms)");
                    stats.chainlink_success = false;
                }
            }
        }
        
        // 2. Para tokens sin precio, intentar pool fallback
        let missing: Vec<_> = tokens_still_needed.iter()
            .filter(|t| !prices.contains_key(t))
            .copied()
            .collect();
        
        if !missing.is_empty() {
            // Usar el pool fallback existente de PriceFeed
            // El método fetch_from_twap_fallback ya está implementado y usa known_prices
            // Necesitamos pasarle los precios conocidos (de Chainlink o cache)
            let known_prices: HashMap<Address, f64> = prices.clone();
            
            // Intentar pool fallback con timeout razonable
            match tokio::time::timeout(
                Duration::from_millis(1000), // 1s para pool fallback (puede tardar ~500ms)
                self.fetch_pool_prices(&missing, &known_prices)
            ).await {
                Ok(Ok(pool_prices)) => {
                    // ✅ FILTRAR precios inválidos antes de agregarlos
                    // Precios deben estar entre $0.0001 y $1,000,000 (rango razonable para tokens)
                    let original_count = pool_prices.len();
                    let valid_pool_prices: HashMap<Address, f64> = pool_prices
                        .into_iter()
                        .filter(|(_, price)| *price > 0.0 && *price >= 0.0001 && *price <= 1_000_000.0)
                        .collect();
                    
                    let filtered_count = original_count - valid_pool_prices.len();
                    if filtered_count > 0 {
                        warn!("⚠️ Pool fallback: Filtered {} invalid prices (out of range)", filtered_count);
                    }
                    
                    stats.pool_count = valid_pool_prices.len();
                    prices.extend(valid_pool_prices);
                }
                Ok(Err(e)) => {
                    warn!("⚠️ Pool fallback failed: {}", e);
                }
                Err(_) => {
                    warn!("⚠️ Pool fallback timeout (1000ms)");
                }
            }
        }
        
        // 3. Hardcoded fallback para stablecoins (último recurso)
        let still_missing: Vec<_> = tokens_to_track.iter()
            .filter(|t| !prices.contains_key(t))
            .copied()
            .collect();
        
        if !still_missing.is_empty() {
            let hardcoded = self.get_hardcoded_prices(&still_missing);
            stats.hardcoded_count = hardcoded.len();
            prices.extend(hardcoded);
        }
        
        // 4. Actualizar cache
        let total_count = prices.len();
        if !prices.is_empty() {
            let source = if stats.chainlink_success {
                PriceSource::Chainlink
            } else if stats.pool_count > 0 {
                PriceSource::PoolBased
            } else {
                PriceSource::Hardcoded
            };
            
            self.cache.update_batch(prices, source);
        }
        
        stats.total_count = total_count;
        stats.duration = start.elapsed();
        
        Ok(stats)
    }
    
    /// Helper para obtener precios desde pools usando el método existente
    async fn fetch_pool_prices(
        &self,
        tokens: &[Address],
        known_prices: &HashMap<Address, f64>,
    ) -> Result<HashMap<Address, f64>> {
        // Usar el método público fetch_from_twap_fallback de PriceFeed
        self.chainlink_feed.fetch_from_twap_fallback(tokens, None, known_prices).await
    }
    
    fn get_hardcoded_prices(&self, tokens: &[Address]) -> HashMap<Address, f64> {
        // Hardcoded SOLO para stablecoins
        // Nota: `.unwrap()` está prohibido fuera de tests (ver `docs/code_conventions.md`).
        // Este método no retorna Result, así que parseamos defensivo y omitimos si hay errores.
        fn parse_addr_or_zero(label: &'static str, s: &'static str) -> Address {
            match s.parse::<Address>() {
                Ok(a) => a,
                Err(e) => {
                    warn!("⚠️ Invalid address constant ({}={}): {:?}", label, s, e);
                    Address::zero()
                }
            }
        }

        let usdc_native = parse_addr_or_zero("USDC_NATIVE", "0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
        // Correct USDC.e on Arbitrum One:
        let usdc_e = parse_addr_or_zero("USDC_E", "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8");
        // Alias histórico encontrado en pools/DB (mantener para hardcoded = 1.0)
        let usdc_legacy = parse_addr_or_zero("USDC_LEGACY", "0xFF970A61A04b1Ca14834A43f5dE4533eBDDB5CC8");
        let usdt = parse_addr_or_zero("USDT", "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");
        let dai = parse_addr_or_zero("DAI", "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1");
        
        let mut prices = HashMap::new();
        for token in tokens {
            let price = match *token {
                addr if addr == usdc_native
                    || addr == usdc_e
                    || addr == usdc_legacy
                    || addr == usdt
                    || addr == dai => Some(1.0),
                _ => None,
            };
            
            if let Some(p) = price {
                prices.insert(*token, p);
            }
        }
        prices
    }
}

#[derive(Default, Debug)]
struct UpdateStats {
    chainlink_success: bool,
    chainlink_count: usize,
    pool_count: usize,
    hardcoded_count: usize,
    total_count: usize,
    duration: Duration,
}

impl std::fmt::Display for UpdateStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Prices: {}/{} (CL:{}, Pool:{}, Hard:{}) in {:?}",
            self.total_count,
            self.chainlink_count + self.pool_count + self.hardcoded_count,
            self.chainlink_count,
            self.pool_count,
            self.hardcoded_count,
            self.duration
        )
    }
}

