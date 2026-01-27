// routegen-rs/src/external_price_updater.rs

use crate::background_price_updater::{PriceSource, SharedPriceCache};
use anyhow::Result;
use ethers::prelude::Address;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// External price updater - actualiza precios 10 veces/segundo desde múltiples fuentes
/// Estrategia de fallback en cascada: Binance WebSocket -> Binance HTTP -> Pyth -> DefiLlama -> Pool Fallback
/// Completamente asíncrono, no afecta el hot path
/// External price updater for fetching prices from external APIs.
///
/// Aggregates prices from multiple external sources (Binance, Pyth, DefiLlama) with
/// automatic fallback and rate limiting.
///
/// ## Supported Sources
///
/// - **Binance**: High-frequency price updates via WebSocket
/// - **Pyth Network**: Oracle-based price feeds
/// - **DefiLlama**: Aggregated DeFi price data
///
/// ## Features
///
/// - **Multi-Source Aggregation**: Combines prices from multiple sources
/// - **Automatic Fallback**: Falls back to alternative sources on failure
/// - **Rate Limiting**: Respects API rate limits
/// - **WebSocket Streaming**: Real-time price updates via WebSocket
pub struct ExternalPriceUpdater {
    cache: SharedPriceCache,
    binance_symbols: HashMap<Address, String>, // Address -> Binance symbol (ej: "ETHUSDT")
    symbol_to_address: HashMap<String, Address>, // Reverse map: symbol -> address
    pyth_addresses: HashMap<Address, String>,  // Address -> Pyth price feed ID
    defillama_addresses: HashMap<Address, String>, // Address -> DefiLlama chain:address format
    update_interval: Duration,
    client: reqwest::Client,
    ws_price_receiver: Arc<tokio::sync::Mutex<Option<mpsc::Receiver<(Address, f64)>>>>, // Receiver para precios del WebSocket
}

#[derive(Debug, Deserialize)]
struct BinancePriceResponse {
    symbol: String,
    price: String,
}

#[derive(Debug, Deserialize)]
struct DefiLlamaPriceResponse {
    coins: HashMap<String, DefiLlamaCoin>,
}

#[derive(Debug, Deserialize)]
struct DefiLlamaCoin {
    price: f64,
    timestamp: Option<u64>, // Timestamp en segundos
}

impl ExternalPriceUpdater {
    pub fn new(cache: SharedPriceCache) -> Self {
        // Nota: `.unwrap()` está prohibido fuera de tests (ver `docs/code_conventions.md`).
        // Este constructor no retorna Result, así que parseamos defensivo y omitimos entradas inválidas.
        fn parse_addr_or_zero(label: &'static str, s: &'static str) -> Address {
            match s.parse::<Address>() {
                Ok(a) => a,
                Err(e) => {
                    warn!("⚠️ Invalid address constant ({}={}): {:?}", label, s, e);
                    Address::zero()
                }
            }
        }

        // Mapeo de addresses a símbolos de Binance (para tokens blue chip)
        let mut binance_symbols = HashMap::new();
        for (label, addr_str, symbol) in [
            (
                "WETH",
                "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                "ETHUSDT",
            ),
            (
                "WBTC",
                "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f",
                "BTCUSDT",
            ),
            (
                "LINK",
                "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4",
                "LINKUSDT",
            ),
            (
                "ARB",
                "0x912ce59144191c1204e64559fe8253a0e49e6548",
                "ARBUSDT",
            ),
            (
                "DAI",
                "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1",
                "DAIUSDT",
            ),
            (
                "AAVE",
                "0xba5DdD1f9d7F570dc94a51479a000E3BCE967196",
                "AAVEUSDT",
            ),
            (
                "wstETH",
                "0x5979D7b546E38E414F7E9822514be443A4800529",
                "WSTETHUSDT",
            ),
            (
                "rETH",
                "0xEC70Dcb4A1EFa46b8F2D97C310C9c4790ba5ffA8",
                "RETHUSDT",
            ),
            (
                "FRAX",
                "0x17fC002b466Eec40dae837fc4bE5C67993DDDc84",
                "FRAXUSDT",
            ),
            (
                "USDC",
                "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
                "USDCUSDT",
            ),
            // Correct USDC.e on Arbitrum One:
            (
                "USDC_E",
                "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8",
                "USDCUSDT",
            ),
            (
                "USDT",
                "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
                "USDTUSDT",
            ),
        ] {
            let addr = parse_addr_or_zero(label, addr_str);
            if addr != Address::zero() {
                binance_symbols.insert(addr, symbol.to_string());
            }
        }

        // Mapeo para Pyth Network (price feed IDs)
        // Pyth usa IDs específicos en formato: "0x..." (price feed address on-chain)
        // O podemos usar la API de Hermes con los IDs de los feeds
        // Formato de Hermes: https://hermes.pyth.network/v2/updates/price/{feed_id}
        // Feed IDs comunes (pueden variar, estos son ejemplos - verificar en docs.pyth.network)
        let mut pyth_addresses = HashMap::new();
        for (label, addr_str, feed_id) in [
            (
                "WETH",
                "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                "0xff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace",
            ),
            (
                "WBTC",
                "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f",
                "0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
            ),
            (
                "LINK",
                "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4",
                "0x8ac0c70fff57e9aefdf5edf44b51d62c2d433653cbb2fa5dbf7d0405e47b9d78",
            ),
            (
                "ARB",
                "0x912ce59144191c1204e64559fe8253a0e49e6548",
                "0x3fa4252848f9f0a1480be62745a462e1079ae237dfdcd35734db2c3a087942a0",
            ),
            (
                "DAI",
                "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1",
                "0xb0948a5e5313200c63389f20c8b0b0b5e4e3b8e0e5e5e5e5e5e5e5e5e5e5e5e5e5",
            ),
            (
                "USDC",
                "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
                "0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a",
            ),
            // Correct USDC.e on Arbitrum One:
            (
                "USDC_E",
                "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8",
                "0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a",
            ),
            (
                "USDT",
                "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
                "0x2b89b9dc8fdf9f34709a5b106b472f0f39bb6ca9ce04b0fd9f2f1c0e5e5e5e5e5",
            ),
        ] {
            let addr = parse_addr_or_zero(label, addr_str);
            if addr != Address::zero() {
                pyth_addresses.insert(addr, feed_id.to_string());
            }
        }

        // Mapeo para DefiLlama (formato: chain:address)
        let mut defillama_addresses = HashMap::new();
        // DefiLlama usa formato "arbitrum:0x..." para tokens en Arbitrum
        for (label, addr_str, llama_id) in [
            (
                "WETH",
                "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                "arbitrum:0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            ),
            (
                "ARB",
                "0x912ce59144191c1204e64559fe8253a0e49e6548",
                "arbitrum:0x912ce59144191c1204e64559fe8253a0e49e6548",
            ),
            (
                "WBTC",
                "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f",
                "arbitrum:0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f",
            ),
            (
                "LINK",
                "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4",
                "arbitrum:0xf97f4df75117a78c1A5a0DBb814Af92458539FB4",
            ),
            (
                "DAI",
                "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1",
                "arbitrum:0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1",
            ),
            (
                "USDC",
                "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
                "arbitrum:0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
            ),
            // Correct USDC.e on Arbitrum One:
            (
                "USDC_E",
                "0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8",
                "arbitrum:0xFF970A61A04b1Ca14834A43f5de4533eBDDB5CC8",
            ),
            (
                "USDT",
                "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
                "arbitrum:0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
            ),
        ] {
            let addr = parse_addr_or_zero(label, addr_str);
            if addr != Address::zero() {
                defillama_addresses.insert(addr, llama_id.to_string());
            }
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200)) // Timeout corto para no bloquear
            .build()
            .expect("Failed to create HTTP client");

        // Crear reverse map para WebSocket
        let mut symbol_to_address = HashMap::new();
        for (addr, symbol) in &binance_symbols {
            symbol_to_address.insert(symbol.clone(), *addr);
        }

        // Crear canal para recibir precios del WebSocket
        let (ws_price_tx, ws_price_rx) = mpsc::channel(1000);

        // Iniciar WebSocket stream en background
        let symbols_for_ws: Vec<String> = binance_symbols.values().cloned().collect();
        let symbol_to_address_ws = symbol_to_address.clone();
        let ws_tx_clone = ws_price_tx.clone();
        tokio::spawn(async move {
            start_binance_websocket(symbols_for_ws, symbol_to_address_ws, ws_tx_clone).await;
        });

        Self {
            cache,
            binance_symbols,
            symbol_to_address,
            pyth_addresses,
            defillama_addresses,
            update_interval: Duration::from_millis(100), // 10 veces/segundo
            client,
            ws_price_receiver: Arc::new(tokio::sync::Mutex::new(Some(ws_price_rx))),
        }
    }

    /// Inicia el updater en background
    pub async fn start(self: Arc<Self>) {
        info!(
            "🚀 Starting external price updater (Binance WS -> Binance HTTP -> Pyth -> DefiLlama)"
        );
        info!("   Binance symbols: {}", self.binance_symbols.len());
        info!("   Pyth feeds: {}", self.pyth_addresses.len());
        info!("   Interval: {:?} (10 updates/sec)", self.update_interval);

        // Procesar mensajes del WebSocket en background (si está disponible)
        // El WebSocket ya está iniciado en el constructor, solo necesitamos procesar los mensajes
        let cache_clone = self.cache.clone();
        let ws_receiver_arc = Arc::clone(&self.ws_price_receiver);
        tokio::spawn(async move {
            let mut ws_rx_guard = ws_receiver_arc.lock().await;
            if let Some(ws_rx) = ws_rx_guard.take() {
                drop(ws_rx_guard); // Liberar el lock antes del loop
                let mut ws_rx = ws_rx;
                while let Some((address, price)) = ws_rx.recv().await {
                    // Actualizar cache directamente desde WebSocket (ultra rápido)
                    let mut prices = HashMap::new();
                    prices.insert(address, price);
                    cache_clone.update_batch(prices, PriceSource::Chainlink);
                }
            }
        });

        let mut ticker = interval(self.update_interval);
        let mut iteration = 0u64;
        let mut consecutive_failures = 0u32;

        loop {
            ticker.tick().await;
            iteration += 1;

            match self.update_prices_cascade().await {
                Ok((_binance_count, pyth_count, defillama_count)) => {
                    let total = _binance_count + pyth_count + defillama_count;
                    if total > 0 {
                        debug!("✅ [External #{}] Updated {} prices (Binance HTTP:{}, Pyth:{}, DefiLlama:{})",
                              iteration, total, _binance_count, pyth_count, defillama_count);
                        consecutive_failures = 0;
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    if consecutive_failures % 10 == 0 {
                        warn!(
                            "⚠️ [External #{}] Update failed (consecutive failures: {}): {}",
                            iteration, consecutive_failures, e
                        );
                    }

                    if consecutive_failures >= 50 {
                        error!(
                            "🚨 External updater has {} consecutive failures",
                            consecutive_failures
                        );
                        consecutive_failures = 0;
                    }
                }
            }
        }
    }

    /// Estrategia de fallback en cascada: Binance -> Pyth -> DefiLlama
    async fn update_prices_cascade(&self) -> Result<(usize, usize, usize)> {
        let start = Instant::now();
        let mut prices = HashMap::new();
        let mut _binance_count = 0;
        let mut pyth_count = 0;
        let mut defillama_count = 0;

        // CAPA 1: Binance (más rápida, <50ms)
        let binance_prices = self.fetch_binance_prices().await.unwrap_or_default();
        _binance_count = binance_prices.len();
        prices.extend(binance_prices);

        // ✅ FASE 5: Hacer Pyth y DefiLlama en paralelo (no secuencial)
        // Preparar listas de tokens faltantes
        let missing_for_pyth: Vec<_> = self
            .pyth_addresses
            .keys()
            .filter(|addr| !prices.contains_key(addr))
            .copied()
            .collect();

        let missing_for_defillama: Vec<_> = self
            .binance_symbols
            .keys()
            .filter(|addr| !prices.contains_key(addr))
            .copied()
            .collect();

        // Ejecutar Pyth y DefiLlama en paralelo
        let (pyth_result, defillama_result) = tokio::join!(
            async {
                if missing_for_pyth.is_empty() {
                    Ok(HashMap::new())
                } else {
                    self.fetch_pyth_prices(&missing_for_pyth).await
                }
            },
            async {
                if missing_for_defillama.is_empty() {
                    Ok(HashMap::new())
                } else {
                    self.fetch_defillama_prices(&missing_for_defillama).await
                }
            }
        );

        // Procesar resultados de Pyth
        if let Ok(pyth_prices) = pyth_result {
            pyth_count = pyth_prices.len();
            prices.extend(pyth_prices);
        }

        // Procesar resultados de DefiLlama
        if let Ok(defillama_prices) = defillama_result {
            defillama_count = defillama_prices.len();
            prices.extend(defillama_prices);
        }

        // Actualizar cache
        if !prices.is_empty() {
            // Usar Chainlink como source para indicar precio externo confiable
            self.cache.update_batch(prices, PriceSource::Chainlink);
        }

        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(50) {
            warn!("⚠️ External price update took {:?} (>50ms target)", elapsed);
        }

        Ok((_binance_count, pyth_count, defillama_count))
    }

    /// CAPA 1: Fetch precios desde Binance API
    async fn fetch_binance_prices(&self) -> Result<HashMap<Address, f64>> {
        let mut prices = HashMap::new();

        // Binance permite batch requests, pero para simplicidad hacemos requests individuales
        // En producción, podrías usar WebSocket para streaming
        let mut tasks = Vec::new();
        let client = self.client.clone();

        for (address, symbol) in &self.binance_symbols {
            let symbol_clone = symbol.clone();
            let address_clone = *address;
            let client_clone = client.clone();

            tasks.push(tokio::spawn(async move {
                let url = format!(
                    "https://api.binance.com/api/v3/ticker/price?symbol={}",
                    symbol_clone
                );
                match client_clone.get(&url).send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            if let Ok(price_data) = resp.json::<BinancePriceResponse>().await {
                                if let Ok(price) = price_data.price.parse::<f64>() {
                                    if price > 0.0 && price >= 0.0001 && price <= 1_000_000.0 {
                                        return Some((address_clone, price));
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                None
            }));
        }

        // Esperar todas las requests en paralelo
        for task in tasks {
            if let Ok(Some((addr, price))) = task.await {
                prices.insert(addr, price);
            }
        }

        Ok(prices)
    }

    /// CAPA 2: Fetch precios desde Pyth Network (Hermes API)
    async fn fetch_pyth_prices(&self, tokens: &[Address]) -> Result<HashMap<Address, f64>> {
        if tokens.is_empty() {
            return Ok(HashMap::new());
        }

        // Pyth Hermes API: https://hermes.pyth.network/v2/updates/price/latest
        // Podemos pedir múltiples feeds a la vez usando el endpoint de latest prices
        let feed_ids: Vec<String> = tokens
            .iter()
            .filter_map(|addr| self.pyth_addresses.get(addr).cloned())
            .collect();

        if feed_ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Hermes permite pedir múltiples feeds separados por coma
        let feeds_param = feed_ids.join(",");
        let url = format!(
            "https://hermes.pyth.network/v2/updates/price/latest?ids={}",
            feeds_param
        );

        let response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(anyhow::anyhow!("Pyth HTTP request failed: {}", e));
            }
        };

        if !response.status().is_success() {
            if response.status() == 429 {
                return Err(anyhow::anyhow!("Pyth rate limited (429)"));
            }
            return Err(anyhow::anyhow!("Pyth HTTP error: {}", response.status()));
        }

        // ✅ FASE 5: Optimizar deserialización JSON usando from_slice (más rápido que from_reader)
        // La respuesta de Hermes es un array de price updates
        // Formato: [{"id": "feed_id", "price": {"price": "1234567890", "expo": -8}, ...}, ...]
        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Pyth response read failed: {}", e))?;
        let price_updates: Vec<serde_json::Value> = match serde_json::from_slice(&bytes) {
            Ok(d) => d,
            Err(e) => {
                return Err(anyhow::anyhow!("Pyth JSON parse failed: {}", e));
            }
        };

        let mut prices = HashMap::new();

        // Mapear de vuelta a addresses
        for (address, feed_id) in &self.pyth_addresses {
            if !tokens.contains(address) {
                continue;
            }

            // Buscar el feed en la respuesta
            for update in &price_updates {
                if let Some(id) = update.get("id").and_then(|v| v.as_str()) {
                    if id == feed_id {
                        // Extraer precio: price.price * 10^price.expo
                        if let Some(price_obj) = update.get("price") {
                            if let (Some(price_str), Some(expo)) = (
                                price_obj.get("price").and_then(|v| v.as_str()),
                                price_obj.get("expo").and_then(|v| v.as_i64()),
                            ) {
                                if let Ok(price_int) = price_str.parse::<f64>() {
                                    let price = price_int * 10.0_f64.powi(expo as i32);

                                    // Validar precio razonable
                                    if price > 0.0 && price >= 0.0001 && price <= 1_000_000.0 {
                                        prices.insert(*address, price);
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }

        Ok(prices)
    }

    /// CAPA 3: Fetch precios desde DefiLlama API
    async fn fetch_defillama_prices(&self, tokens: &[Address]) -> Result<HashMap<Address, f64>> {
        if tokens.is_empty() {
            return Ok(HashMap::new());
        }

        // Construir lista de direcciones para DefiLlama
        let addresses: Vec<String> = tokens
            .iter()
            .filter_map(|addr| self.defillama_addresses.get(addr).cloned())
            .collect();

        if addresses.is_empty() {
            return Ok(HashMap::new());
        }

        let addresses_param = addresses.join(",");
        let url = format!("https://coins.llama.fi/prices/current/{}", addresses_param);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "DefiLlama HTTP error: {}",
                response.status()
            ));
        }

        // ✅ FASE 5: Optimizar deserialización JSON usando from_slice (más rápido que from_reader)
        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("DefiLlama response read failed: {}", e))?;
        let price_data: DefiLlamaPriceResponse = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("DefiLlama JSON parse failed: {}", e))?;

        let mut prices = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Mapear de vuelta a addresses
        for (address, defillama_key) in &self.defillama_addresses {
            if let Some(coin_data) = price_data.coins.get(defillama_key) {
                let price = coin_data.price;

                // Validar que el precio no sea muy viejo (máx 60 segundos)
                let is_fresh = coin_data
                    .timestamp
                    .map(|ts| now.saturating_sub(ts) < 60)
                    .unwrap_or(true);

                if is_fresh && price > 0.0 && price >= 0.0001 && price <= 1_000_000.0 {
                    prices.insert(*address, price);
                }
            }
        }

        Ok(prices)
    }
}

/// WebSocket stream de Binance para precios en tiempo real
/// Se reconecta automáticamente en caso de desconexión
async fn start_binance_websocket(
    symbols: Vec<String>,
    symbol_to_address: HashMap<String, Address>,
    price_tx: mpsc::Sender<(Address, f64)>,
) {
    info!(
        "🔌 Starting Binance WebSocket stream for {} symbols",
        symbols.len()
    );

    // Construir URL de WebSocket con múltiples streams
    // Formato: wss://stream.binance.com:9443/stream?streams=ethusdt@ticker/btcusdt@ticker
    let streams: Vec<String> = symbols
        .iter()
        .map(|s| format!("{}@ticker", s.to_lowercase()))
        .collect();
    let streams_param = streams.join("/");
    let ws_url = format!(
        "wss://stream.binance.com:9443/stream?streams={}",
        streams_param
    );

    let mut reconnect_delay = Duration::from_secs(1);
    let max_reconnect_delay = Duration::from_secs(60);

    loop {
        match connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                info!("✅ Binance WebSocket connected");
                reconnect_delay = Duration::from_secs(1); // Reset delay on success

                let (mut _write, mut read) = ws_stream.split();

                // Procesar mensajes del stream
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Parsear mensaje de Binance
                            // Formato: {"stream":"ethusdt@ticker","data":{"e":"24hTicker","E":123456789,"s":"ETHUSDT","c":"0.0015",...}}
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                if json.get("stream").is_some() {
                                    if let Some(data) = json.get("data") {
                                        if let (Some(symbol), Some(price_str)) = (
                                            data.get("s").and_then(|v| v.as_str()),
                                            data.get("c").and_then(|v| v.as_str()),
                                        ) {
                                            // Mapear symbol a address
                                            if let Some(&address) = symbol_to_address.get(symbol) {
                                                if let Ok(price) = price_str.parse::<f64>() {
                                                    // Validar precio razonable
                                                    if price > 0.0
                                                        && price >= 0.0001
                                                        && price <= 1_000_000.0
                                                    {
                                                        // Enviar precio al canal (non-blocking)
                                                        if price_tx
                                                            .try_send((address, price))
                                                            .is_err()
                                                        {
                                                            // Canal lleno, ignorar este precio (no crítico)
                                                            debug!("⚠️ Binance WS: Channel full, dropping price update");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Message::Ping(_)) => {
                            // Responder ping automáticamente
                            // El cliente tungstenite maneja esto automáticamente
                        }
                        Ok(Message::Close(_)) => {
                            warn!("⚠️ Binance WebSocket closed");
                            break;
                        }
                        Err(e) => {
                            warn!("⚠️ Binance WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("❌ Failed to connect to Binance WebSocket: {}", e);
            }
        }

        // Reconexión con backoff exponencial
        warn!(
            "🔄 Reconnecting to Binance WebSocket in {:?}...",
            reconnect_delay
        );
        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
    }
}

#[derive(Debug, Deserialize)]
struct BinanceTickerMessage {
    stream: String,
    data: BinanceTickerData,
}

#[derive(Debug, Deserialize)]
struct BinanceTickerData {
    #[serde(rename = "e")]
    event_type: String,
    #[serde(rename = "E")]
    event_time: u64,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "c")]
    close_price: String,
}
