// Pool Event Extractor - Extrae eventos de creación de pools de bloques
// Identifica PairCreated (V2) y PoolCreated (V3) events de los logs del bloque
// Puede usar logs ya obtenidos o obtenerlos vía RPC si es necesario

use anyhow::Result;
use ethers::prelude::{Address, Http, Middleware, Provider};
use ethers::types::{Block, Filter, Transaction};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::contracts::{
    i_uniswap_v2_factory::PairCreatedFilter, i_uniswap_v3_factory::PoolCreatedFilter,
};
use crate::rpc_pool::RpcPool;

/// Candidato de pool descubierto desde eventos del bloque
#[derive(Debug, Clone)]
pub struct PoolCandidate {
    pub address: Address,
    pub dex: String,
    pub factory: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee_bps: Option<u32>, // Para V3 pools
    pub discovered_at_block: u64,
}

/// Mapeo de factory addresses a nombres de DEX
/// Se inicializa desde Settings en runtime
#[derive(Clone)]
pub struct FactoryToDexMap {
    factories: HashMap<Address, String>,
}

impl FactoryToDexMap {
    /// Obtiene todas las factory addresses
    pub fn factory_addresses(&self) -> Vec<Address> {
        self.factories.keys().copied().collect()
    }

    /// Obtiene el número de factories
    pub fn len(&self) -> usize {
        self.factories.len()
    }
}

impl FactoryToDexMap {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Agrega un mapeo factory -> DEX
    pub fn add_factory(&mut self, factory: Address, dex_name: String) {
        self.factories.insert(factory, dex_name);
    }

    /// Obtiene el nombre del DEX para una factory address
    pub fn get_dex(&self, factory: Address) -> Option<&String> {
        self.factories.get(&factory)
    }
}

impl Default for FactoryToDexMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Extrae eventos de creación de pools de un bloque
/// Si se proporciona un rpc_pool, obtiene logs vía RPC con registro en Flight Recorder. Si no, solo extrae de tx.to
pub async fn extract_pool_creation_events(
    block: &Block<Transaction>,
    factory_map: &FactoryToDexMap,
    rpc_pool: Option<Arc<RpcPool>>,
) -> Result<Vec<PoolCandidate>> {
    let block_number = block
        .number
        .ok_or_else(|| anyhow::anyhow!("Block number not available"))?
        .as_u64();

    let mut candidates = Vec::new();

    // Método 1: Extraer de tx.to (sin logs, sin RPC)
    // Identificar transacciones que llaman a factories conocidas
    for tx in &block.transactions {
        let tx_to = match &tx.to {
            Some(addr) => *addr,
            None => continue,
        };

        // Si es una factory conocida, podría haber creado un pool
        // Pero sin logs, no podemos saber el address del pool creado
        // Por ahora, solo usamos este método como hint
        if factory_map.get_dex(tx_to).is_some() {
            debug!(
                "🔍 [EventExtractor] Found transaction to known factory {} in block {}",
                tx_to, block_number
            );
        }
    }

    // Método 2: Obtener eventos vía RPC usando get_logs() combinado (OPTIMIZADO: 1 call en lugar de 16-20)
    // ✅ OPTIMIZACIÓN: Combinar todas las factories en un solo get_logs() con filtro múltiple
    // ✅ FLIGHT RECORDER: Usar RpcPool para registrar todas las llamadas eth_getLogs
    if let Some(rpc_pool_ref) = rpc_pool {
        use ethers::types::{H256, U256};
        use std::str::FromStr;

        let factory_addresses: Vec<Address> = factory_map.factory_addresses();

        // Event signatures
        // PairCreated(address indexed token0, address indexed token1, address pair, uint)
        let pair_created_sig =
            H256::from_str("0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9")
                .unwrap_or_else(|_| H256::zero());
        // PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickLower, int24 tickUpper, uint160 sqrtPriceX96, uint128 liquidity, address pool)
        let pool_created_sig =
            H256::from_str("0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118")
                .unwrap_or_else(|_| H256::zero());

        // Validar límites RPC (típicamente 50-200 addresses por filtro)
        const MAX_ADDRESSES_PER_FILTER: usize = 100;

        if factory_addresses.is_empty() {
            debug!("⚠️ [EventExtractor] No factory addresses available");
        } else if factory_addresses.len() <= MAX_ADDRESSES_PER_FILTER {
            // ✅ OPTIMIZADO: Una sola llamada get_logs() para todas las factories
            // ✅ FLIGHT RECORDER: Usar RpcPool para registrar la llamada
            let filter = Filter::new()
                .from_block(block_number)
                .to_block(block_number)
                .address(factory_addresses.clone())
                .topic0(vec![pair_created_sig, pool_created_sig]);

            // Obtener provider y endpoint para registro
            let (provider, _permit, endpoint) =
                rpc_pool_ref.get_next_provider_with_endpoint().await?;
            match rpc_pool_ref
                .get_logs_with_recording(&provider, &filter, &endpoint)
                .await
            {
                Ok(logs) => {
                    // ✅ OPTIMIZATION: Registrar métrica RPC
                    crate::metrics::increment_rpc_call("streaming_discovery");
                    crate::metrics::set_rpc_calls_per_block("streaming_discovery", 1.0);

                    info!("📊 [EventExtractor] Fetched {} logs in 1 RPC call (optimized from {} calls)",
                          logs.len(), factory_addresses.len() * 2);

                    for log in logs {
                        // Identificar factory desde log.address
                        let factory_addr = log.address;
                        if let Some(dex_name) = factory_map.get_dex(factory_addr) {
                            // Identificar tipo de evento desde topic0
                            if let Some(topic0) = log.topics.get(0) {
                                if *topic0 == pair_created_sig {
                                    // V2 PairCreated event
                                    if log.topics.len() >= 3 {
                                        let token0 =
                                            Address::from_slice(&log.topics[1].as_bytes()[12..]);
                                        let token1 =
                                            Address::from_slice(&log.topics[2].as_bytes()[12..]);
                                        let pair = Address::from_slice(&log.data.as_ref()[12..32]);

                                        candidates.push(PoolCandidate {
                                            address: pair,
                                            dex: dex_name.clone(),
                                            factory: factory_addr,
                                            token0,
                                            token1,
                                            fee_bps: Some(30), // Default V2 fee
                                            discovered_at_block: block_number,
                                        });
                                        debug!(
                                            "🔍 [EventExtractor] Found V2 pool: {} (DEX: {}, tokens: {:?}/{:?})",
                                            pair, dex_name, token0, token1
                                        );
                                    }
                                } else if *topic0 == pool_created_sig {
                                    // V3 PoolCreated event
                                    if log.topics.len() >= 4 && log.data.len() >= 128 {
                                        let token0 =
                                            Address::from_slice(&log.topics[1].as_bytes()[12..]);
                                        let token1 =
                                            Address::from_slice(&log.topics[2].as_bytes()[12..]);
                                        let fee = U256::from_big_endian(
                                            &log.topics[3].as_bytes()[29..32],
                                        );
                                        let pool = Address::from_slice(&log.data.as_ref()[12..32]);

                                        candidates.push(PoolCandidate {
                                            address: pool,
                                            dex: dex_name.clone(),
                                            factory: factory_addr,
                                            token0,
                                            token1,
                                            fee_bps: Some(fee.as_u32()),
                                            discovered_at_block: block_number,
                                        });
                                        debug!(
                                            "🔍 [EventExtractor] Found V3 pool: {} (DEX: {}, tokens: {:?}/{:?}, fee: {} bps)",
                                            pool, dex_name, token0, token1, fee.as_u32()
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️ [EventExtractor] Failed to fetch combined logs: {} (falling back to individual queries)", e);
                    // Fallback: intentar queries individuales si get_logs() combinado falla
                    // (mantener compatibilidad, aunque debería ser raro)
                    for factory_addr in factory_addresses {
                        if let Some(dex_name) = factory_map.get_dex(factory_addr) {
                            // Obtener nuevo provider para fallback
                            let (provider_fallback, _permit_fallback) =
                                rpc_pool_ref.get_next_provider().await?;
                            // Intentar V2
                            let v2_factory = crate::contracts::IUniswapV2Factory::new(
                                factory_addr,
                                provider_fallback.clone(),
                            );
                            let v2_event_filter = v2_factory
                                .event::<PairCreatedFilter>()
                                .from_block(block_number)
                                .to_block(block_number);

                            if let Ok(decoded_logs) = v2_event_filter.query().await {
                                for decoded_log in decoded_logs {
                                    candidates.push(PoolCandidate {
                                        address: decoded_log.pair,
                                        dex: dex_name.clone(),
                                        factory: factory_addr,
                                        token0: decoded_log.token_0,
                                        token1: decoded_log.token_1,
                                        fee_bps: Some(30),
                                        discovered_at_block: block_number,
                                    });
                                }
                            }

                            // Intentar V3
                            let v3_factory = crate::contracts::IUniswapV3Factory::new(
                                factory_addr,
                                provider_fallback.clone(),
                            );
                            let v3_event_filter = v3_factory
                                .event::<PoolCreatedFilter>()
                                .from_block(block_number)
                                .to_block(block_number);

                            if let Ok(decoded_logs) = v3_event_filter.query().await {
                                for decoded_log in decoded_logs {
                                    candidates.push(PoolCandidate {
                                        address: decoded_log.pool,
                                        dex: dex_name.clone(),
                                        factory: factory_addr,
                                        token0: decoded_log.token_0,
                                        token1: decoded_log.token_1,
                                        fee_bps: Some(decoded_log.fee),
                                        discovered_at_block: block_number,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        } else {
            // Dividir en múltiples filtros si excede límite
            warn!(
                "⚠️ [EventExtractor] Too many factories ({}), splitting into multiple filters",
                factory_addresses.len()
            );
            for chunk in factory_addresses.chunks(MAX_ADDRESSES_PER_FILTER) {
                let filter = Filter::new()
                    .from_block(block_number)
                    .to_block(block_number)
                    .address(chunk.to_vec())
                    .topic0(vec![pair_created_sig, pool_created_sig]);

                // ✅ FLIGHT RECORDER: Usar RpcPool para registrar cada llamada
                let (provider_chunk, _permit_chunk, endpoint_chunk) =
                    rpc_pool_ref.get_next_provider_with_endpoint().await?;
                match rpc_pool_ref
                    .get_logs_with_recording(&provider_chunk, &filter, &endpoint_chunk)
                    .await
                {
                    Ok(logs) => {
                        // Parsear logs igual que arriba
                        for log in logs {
                            let factory_addr = log.address;
                            if let Some(dex_name) = factory_map.get_dex(factory_addr) {
                                if let Some(topic0) = log.topics.get(0) {
                                    if *topic0 == pair_created_sig && log.topics.len() >= 3 {
                                        let token0 =
                                            Address::from_slice(&log.topics[1].as_bytes()[12..]);
                                        let token1 =
                                            Address::from_slice(&log.topics[2].as_bytes()[12..]);
                                        let pair = Address::from_slice(&log.data.as_ref()[12..32]);
                                        candidates.push(PoolCandidate {
                                            address: pair,
                                            dex: dex_name.clone(),
                                            factory: factory_addr,
                                            token0,
                                            token1,
                                            fee_bps: Some(30),
                                            discovered_at_block: block_number,
                                        });
                                    } else if *topic0 == pool_created_sig
                                        && log.topics.len() >= 4
                                        && log.data.len() >= 128
                                    {
                                        let token0 =
                                            Address::from_slice(&log.topics[1].as_bytes()[12..]);
                                        let token1 =
                                            Address::from_slice(&log.topics[2].as_bytes()[12..]);
                                        let fee = U256::from_big_endian(
                                            &log.topics[3].as_bytes()[29..32],
                                        );
                                        let pool = Address::from_slice(&log.data.as_ref()[12..32]);
                                        candidates.push(PoolCandidate {
                                            address: pool,
                                            dex: dex_name.clone(),
                                            factory: factory_addr,
                                            token0,
                                            token1,
                                            fee_bps: Some(fee.as_u32()),
                                            discovered_at_block: block_number,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [EventExtractor] Failed to fetch logs for chunk: {}", e);
                    }
                }
            }
        }
    }

    if !candidates.is_empty() {
        info!(
            "📊 [EventExtractor] Extracted {} pool candidates from block {}",
            candidates.len(),
            block_number
        );
    }

    Ok(candidates)
}

/// Crea un FactoryToDexMap desde Settings
pub fn create_factory_map_from_settings(settings: &crate::settings::Settings) -> FactoryToDexMap {
    let mut map = FactoryToDexMap::new();

    // Uniswap V2
    if let Ok(factory) = settings.contracts.factories.uniswap_v2.parse::<Address>() {
        map.add_factory(factory, "UniswapV2".to_string());
    }

    // Uniswap V3
    if let Ok(factory) = settings.contracts.factories.uniswap_v3.parse::<Address>() {
        map.add_factory(factory, "UniswapV3".to_string());
    }

    // SushiSwap V2 (mismo formato que UniswapV2)
    if let Ok(factory) = settings.contracts.factories.sushiswap_v2.parse::<Address>() {
        map.add_factory(factory, "SushiSwapV2".to_string());
    }

    // Camelot V2
    if let Ok(factory) = settings.contracts.factories.camelot_v2.parse::<Address>() {
        map.add_factory(factory, "CamelotV2".to_string());
    }

    // Camelot V3
    if let Ok(factory) = settings.contracts.factories.camelot_v3.parse::<Address>() {
        map.add_factory(factory, "CamelotV3".to_string());
    }

    // PancakeSwap V2
    if let Ok(factory) = settings
        .contracts
        .factories
        .pancakeswap_v2
        .parse::<Address>()
    {
        map.add_factory(factory, "PancakeSwapV2".to_string());
    }

    // TraderJoe V2
    if let Ok(factory) = settings.contracts.factories.traderjoe_v2.parse::<Address>() {
        map.add_factory(factory, "TraderJoeV2".to_string());
    }

    // KyberSwap Elastic (V3-like)
    if let Ok(factory) = settings
        .contracts
        .factories
        .kyberswap_elastic
        .parse::<Address>()
    {
        map.add_factory(factory, "KyberSwapV3".to_string());
    }

    info!(
        "✅ [EventExtractor] Factory map created with {} factories",
        map.len()
    );
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::{Log, H256, U256, U64};
    use std::str::FromStr;

    fn create_test_log(address: Address, topics: Vec<H256>, data: Vec<u8>) -> Log {
        Log {
            address,
            topics,
            data: data.into(),
            block_number: Some(U64::from(100)),
            block_hash: Some(
                H256::from_str(
                    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                )
                .unwrap(),
            ),
            transaction_hash: Some(
                H256::from_str(
                    "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                )
                .unwrap(),
            ),
            transaction_index: Some(U64::from(0)),
            log_index: Some(U256::from(0)),
            removed: Some(false),
            log_type: None,
            transaction_log_index: None,
        }
    }

    #[test]
    fn test_factory_map() {
        let mut map = FactoryToDexMap::new();
        let factory = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();
        map.add_factory(factory, "TestDEX".to_string());

        assert_eq!(map.get_dex(factory), Some(&"TestDEX".to_string()));
        assert_eq!(map.get_dex(Address::zero()), None);
    }
}
