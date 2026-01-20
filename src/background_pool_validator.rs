// Background Pool Validator - Valida pools en background y mantiene cache warm
// Ejecuta validaciones continuamente sin bloquear la generación de rutas

use anyhow::{anyhow, Result};
use ethers::prelude::Address;
use ethers::types::U256;
use ethers::providers::Middleware;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::sleep;
use log::{info, warn, error};

use crate::database::{load_valid_pools_by_liquidity_range, DbPool};
use crate::jit_state_fetcher::{JitStateFetcher, PoolMetadata, PoolType, FreshPoolStates};
use crate::pools::Pool;
use crate::pool_validation_cache::PoolValidationCache;
use crate::rpc_pool::RpcPool;
use crate::settings::Settings;
use crate::block_parser::BlockParser; // 🚀 RPC OPTIMIZATION: Block-based filtering

pub struct BackgroundPoolValidator {
    pool_cache: Arc<PoolValidationCache>,
    rpc_pool: Arc<RpcPool>,
    db_pool: Arc<DbPool>,
    settings: Arc<Settings>,
    multicall_address: Address,
    current_block: Arc<AtomicU64>,
    block_parser: Option<Arc<BlockParser>>, // 🚀 RPC OPTIMIZATION: Block-based filtering
}

impl BackgroundPoolValidator {
    pub fn new(
        pool_cache: Arc<PoolValidationCache>,
        rpc_pool: Arc<RpcPool>,
        db_pool: Arc<DbPool>,
        settings: Arc<Settings>,
        multicall_address: Address,
    ) -> Self {
        Self {
            pool_cache,
            rpc_pool,
            db_pool,
            settings,
            multicall_address,
            current_block: Arc::new(AtomicU64::new(0)),
            block_parser: None,
        }
    }

    /// 🚀 RPC OPTIMIZATION: Set block parser for block-based filtering
    pub fn with_block_parser(mut self, block_parser: Arc<BlockParser>) -> Self {
        self.block_parser = Some(block_parser);
        self
    }

    /// Iniciar background validator con frecuencia adaptativa
    pub async fn start(&self) -> tokio::task::JoinHandle<()> {
        let validator = BackgroundValidatorTask {
            pool_cache: self.pool_cache.clone(),
            rpc_pool: self.rpc_pool.clone(),
            db_pool: self.db_pool.clone(),
            settings: self.settings.clone(),
            multicall_address: self.multicall_address,
            current_block: self.current_block.clone(),
            block_parser: self.block_parser.clone(),
        };
        
        tokio::spawn(async move {
            validator.run_loop().await;
        })
    }

    /// Validar pools en batch (para uso externo, ej. warm-up)
    pub async fn validate_pools_batch(
        &self,
        pools: &[Pool],
        current_block: u64,
    ) -> Result<usize> {
        validate_pools_batch_impl(
            pools,
            self.pool_cache.clone(),
            self.rpc_pool.clone(),
            self.settings.clone(),
            self.multicall_address,
            current_block,
        ).await
    }

    /// Obtener bloque actual
    pub async fn get_current_block(&self) -> Result<u64> {
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        let block = provider.get_block_number().await?;
        Ok(block.as_u64())
    }
}

struct BackgroundValidatorTask {
    pool_cache: Arc<PoolValidationCache>,
    rpc_pool: Arc<RpcPool>,
    db_pool: Arc<DbPool>,
    settings: Arc<Settings>,
    multicall_address: Address,
    current_block: Arc<AtomicU64>,
    block_parser: Option<Arc<BlockParser>>, // 🚀 RPC OPTIMIZATION: Block-based filtering
}

impl BackgroundValidatorTask {
    async fn run_loop(&self) {
        let mut iteration = 0;
        
        loop {
            iteration += 1;
            let loop_start = Instant::now();
            
            // 1. Obtener bloque actual
            let current_block = match self.get_current_block().await {
                Ok(block) => {
                    self.current_block.store(block, Ordering::Relaxed);
                    block
                }
                Err(e) => {
                    error!("❌ Failed to get current block: {}", e);
                    sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            
            // 2. Invalidar pools stale del cache
            let stale_removed = self.pool_cache.invalidate_stale(current_block);
            if stale_removed > 0 {
                info!("🗑️ Removed {} stale pool validations from cache", stale_removed);
            }
            
            // 3. Cargar pools de DB
            let pools_start = Instant::now();
            let pools = match self.load_pools_from_db().await {
                Ok(pools) => pools,
                Err(e) => {
                    error!("❌ Failed to load pools from DB: {}", e);
                    sleep(self.calculate_next_interval(Duration::from_secs(10))).await;
                    continue;
                }
            };
            info!("📦 Loaded {} pools from DB in {:?}", pools.len(), pools_start.elapsed());
            
            // 4. Pre-filtrado adaptativo con block-based filtering (reducir pools a validar)
            let pre_filtered = if let Some(ref block_parser) = self.block_parser {
                // 🚀 RPC OPTIMIZATION: Filtrar pools usando block parser
                let filtered = self.pre_filter_pools_with_block_parser(&pools, current_block, block_parser).await;
                let reduction_pct = if pools.len() > 0 {
                    (1.0 - (filtered.len() as f64 / pools.len() as f64)) * 100.0
                } else {
                    0.0
                };
                info!("🔍 [BlockFilter] Filtered {} pools to {} pools ({}% reduction)", 
                      pools.len(), filtered.len(), reduction_pct);
                filtered
            } else {
                // Sin block parser, usar filtrado adaptativo normal
                let filtered = self.pre_filter_pools_adaptive(&pools, current_block);
                info!("🔍 Pre-filtered to {} high-quality pools", filtered.len());
                filtered
            };
            
            // 5. Validar pools en batches concurrentes
            let validation_start = Instant::now();
            match self.validate_pools_batch(&pre_filtered, current_block).await {
                Ok(validated_count) => {
                    info!("✅ Validated {} pools in {:?}", validated_count, validation_start.elapsed());
                }
                Err(e) => {
                    warn!("⚠️ Pool validation failed: {}", e);
                }
            }
            
            // 6. Log métricas del cache
            let cache_metrics = self.pool_cache.metrics();
            info!("📊 Cache metrics: hit_rate={:.1}%, size={}, hits={}, misses={}", 
                  cache_metrics.hit_rate * 100.0, cache_metrics.cache_size, cache_metrics.hits, cache_metrics.misses);
            
            // 7. Calcular tiempo de loop y ajustar intervalo
            let loop_duration = loop_start.elapsed();
            let next_interval = self.calculate_next_interval(loop_duration);
            
            info!("🔄 Background validation iteration {} completed in {:?}, next in {:?}", 
                  iteration, loop_duration, next_interval);
            
            sleep(next_interval).await;
        }
    }

    /// Calcular intervalo adaptativo según carga (OPTIMIZADO: frecuencia reducida a 5 minutos)
    fn calculate_next_interval(&self, last_duration: Duration) -> Duration {
        // 🚀 RPC OPTIMIZATION: Reducir frecuencia de 60-120s a 300-600s (5-10 minutos)
        // Con block-based filtering, solo validamos pools tocados, así que podemos reducir frecuencia
        if last_duration > Duration::from_secs(10) {
            Duration::from_secs(600) // RPC cargado - 10 minutos
        } else if last_duration > Duration::from_secs(5) {
            Duration::from_secs(450) // RPC moderado - 7.5 minutos
        } else {
            Duration::from_secs(300) // RPC libre - 5 minutos mínimo
        }
    }

    /// Obtener bloque actual desde RPC
    async fn get_current_block(&self) -> Result<u64> {
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        let block = provider.get_block_number().await?;
        Ok(block.as_u64())
    }

    /// Cargar pools de DB con múltiples tiers de liquidez
    async fn load_pools_from_db(&self) -> Result<Vec<Pool>> {
        // Cargar pools de alta liquidez primero
        let high_liq = load_valid_pools_by_liquidity_range(
            &self.db_pool,
            100_000.0,
            10_000_000.0,
        ).await?;
        
        // Si hay pocos pools de alta liquidez, expandir
        if high_liq.len() < 200 {
            let medium_liq = load_valid_pools_by_liquidity_range(
                &self.db_pool,
                50_000.0,
                100_000.0,
            ).await?;
            
            let mut all_pools = high_liq;
            all_pools.extend(medium_liq);
            
            if all_pools.len() < 200 {
                let low_liq = load_valid_pools_by_liquidity_range(
                    &self.db_pool,
                    25_000.0,
                    50_000.0,
                ).await?;
                all_pools.extend(low_liq);
            }
            
            Ok(all_pools)
        } else {
            Ok(high_liq)
        }
    }

    /// 🚀 RPC OPTIMIZATION: Pre-filtrado con block parser (solo pools tocados)
    async fn pre_filter_pools_with_block_parser(
        &self,
        pools: &[Pool],
        current_block: u64,
        block_parser: &BlockParser,
    ) -> Vec<Pool> {
        // Obtener pools tocados en últimos 5 bloques
        let (provider, _permit) = match self.rpc_pool.get_next_provider().await {
            Ok(p) => p,
            Err(_) => {
                warn!("⚠️ Failed to get provider for block filtering, using all pools");
                return self.pre_filter_pools_adaptive(pools, current_block);
            }
        };
        
        let mut touched_pools = std::collections::HashSet::new();
        for i in 1..=5 {
            let block_num = current_block.saturating_sub(i);
            if let Ok(Some(block)) = block_parser.get_block_with_timeout(provider.clone(), block_num).await {
                let basic_touched = block_parser.extract_touched_pools_basic(&block);
                touched_pools.extend(basic_touched);
            }
        }
        
        // Filtrar: solo pools que fueron tocados
        let filtered: Vec<Pool> = pools.iter()
            .filter(|p| touched_pools.contains(&p.address()))
            .cloned()
            .collect();
        
        // Si hay muy pocos pools filtrados, usar filtrado adaptativo normal (fallback)
        if filtered.is_empty() && pools.len() > 0 {
            warn!("⚠️ [BlockFilter] No pools touched, using adaptive filtering as fallback");
            self.pre_filter_pools_adaptive(pools, current_block)
        } else {
            filtered
        }
    }

    /// Pre-filtrado adaptativo de pools
    fn pre_filter_pools_adaptive(&self, pools: &[Pool], current_block: u64) -> Vec<Pool> {
        const TARGET_POOL_COUNT: usize = 300;
        
        // Empezar con filtros estrictos
        let mut filtered: Vec<_> = pools.iter()
            .filter(|p| self.is_pool_high_quality(p, current_block))
            .cloned()
            .collect();
        
        // Si no hay suficientes, relajar filtros
        if filtered.len() < TARGET_POOL_COUNT {
            filtered = pools.iter()
                .filter(|p| self.is_pool_medium_quality(p, current_block))
                .cloned()
                .collect();
        }
        
        // Si aún no hay suficientes, relajar más
        if filtered.len() < TARGET_POOL_COUNT {
            filtered = pools.iter()
                .filter(|p| self.is_pool_minimum_quality(p, current_block))
                .cloned()
                .collect();
        }
        
        // Limitar a TARGET_POOL_COUNT para evitar validar demasiados
        filtered.truncate(TARGET_POOL_COUNT);
        filtered
    }

    fn is_pool_high_quality(&self, pool: &Pool, current_block: u64) -> bool {
        match pool {
            Pool::UniswapV2(_) => {
                // AJUSTADO: Relajar requisito de recency (200 → 500 bloques)
                // Solo verificar blacklist, no whitelist (menos restrictivo)
                // Nota: last_updated_block no está disponible directamente en Pool
                // Necesitamos obtenerlo de DB o usar otro criterio
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1()) &&
                pool.reserve0() > U256::zero() && 
                pool.reserve1() > U256::zero()
            }
            Pool::UniswapV3(p) => {
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1()) &&
                [100u32, 500u32, 3000u32, 10000u32].contains(&p.fee) &&
                p.liquidity > 0
            }
            _ => false,
        }
    }

    fn is_pool_medium_quality(&self, pool: &Pool, current_block: u64) -> bool {
        match pool {
            Pool::UniswapV2(_) => {
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1()) &&
                (pool.reserve0() > U256::zero() || pool.reserve1() > U256::zero())
            }
            Pool::UniswapV3(p) => {
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1()) &&
                p.liquidity > 0
            }
            _ => false,
        }
    }

    fn is_pool_minimum_quality(&self, pool: &Pool, current_block: u64) -> bool {
        match pool {
            Pool::UniswapV2(_) => {
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1())
            }
            Pool::UniswapV3(_) => {
                !self.is_token_blacklisted(pool.token0()) &&
                !self.is_token_blacklisted(pool.token1())
            }
            _ => false,
        }
    }

    fn is_token_blacklisted(&self, token: Address) -> bool {
        // ✅ IMPLEMENTADO: Cargar desde settings.validator.blacklisted_tokens
        use std::str::FromStr;
        
        // Parsear tokens blacklisted desde settings (strings a Address)
        for token_str in &self.settings.validator.blacklisted_tokens {
            if let Ok(blacklisted_addr) = Address::from_str(token_str) {
                if blacklisted_addr == token {
                    return true;
                }
            }
        }
        
        false
    }

    /// Validar pools en batches concurrentes
    async fn validate_pools_batch(&self, pools: &[Pool], current_block: u64) -> Result<usize> {
        validate_pools_batch_impl(
            pools,
            self.pool_cache.clone(),
            self.rpc_pool.clone(),
            self.settings.clone(),
            self.multicall_address,
            current_block,
        ).await
    }
}

/// Validar un batch de pools usando JIT fetcher con retry
async fn validate_pools_batch_impl(
    pools: &[Pool],
    cache: Arc<PoolValidationCache>,
    rpc_pool: Arc<RpcPool>,
    settings: Arc<Settings>,
    multicall_address: Address,
    current_block: u64,
) -> Result<usize> {
    // 🚀 RPC OPTIMIZATION: Incrementar batch size de 50 a 200
    const BATCH_SIZE: usize = 200;
    const MAX_CONCURRENT_BATCHES: usize = 3; // Reducir batches concurrentes ya que cada batch es más grande
    
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_BATCHES));
    let mut validation_tasks = Vec::new();
    
    // Dividir pools en batches
    for batch in pools.chunks(BATCH_SIZE) {
        let batch = batch.to_vec();
        let cache_clone = cache.clone();
        let rpc_pool_clone = rpc_pool.clone();
        let settings_clone = settings.clone();
        let permit = semaphore.clone();
        
        let task = tokio::spawn(async move {
            let _permit = permit.acquire().await.unwrap();
            validate_pool_batch(
                batch,
                cache_clone,
                rpc_pool_clone,
                settings_clone,
                multicall_address,
                current_block,
            ).await
        });
        
        validation_tasks.push(task);
    }
    
    // Esperar todas las validaciones
    let mut validated_count = 0;
    for task in validation_tasks {
        match task.await {
            Ok(Ok(count)) => validated_count += count,
            Ok(Err(e)) => warn!("⚠️ Batch validation failed: {}", e),
            Err(e) => warn!("⚠️ Validation task panicked: {}", e),
        }
    }
    
    Ok(validated_count)
}

/// Validar un batch de pools usando JIT fetcher
async fn validate_pool_batch(
    pools: Vec<Pool>,
    cache: Arc<PoolValidationCache>,
    rpc_pool: Arc<RpcPool>,
    settings: Arc<Settings>,
    multicall_address: Address,
    current_block: u64,
) -> Result<usize> {
    // 1. Extraer metadata de pools
    let pool_metadata: Vec<PoolMetadata> = pools.iter()
        .filter_map(|p| {
            match p {
                Pool::UniswapV2(v2) => Some(PoolMetadata {
                    address: v2.address,
                    pool_type: PoolType::V2,
                    token0: v2.token0,
                    token1: v2.token1,
                    fee: None,
                }),
                Pool::UniswapV3(v3) => Some(PoolMetadata {
                    address: v3.address,
                    pool_type: PoolType::V3,
                    token0: v3.token0,
                    token1: v3.token1,
                    fee: Some(v3.fee),
                }),
                _ => None,
            }
        })
        .collect();
    
    if pool_metadata.is_empty() {
        return Ok(0);
    }
    
    // 2. Fetchar estados frescos usando JIT con retry
    let jit_fetcher = JitStateFetcher::new(
        rpc_pool,
        multicall_address,
        settings.performance.multicall_batch_size,
        settings,
    );
    
    let fresh_states = fetch_with_retry(
        &jit_fetcher,
        &pool_metadata,
        current_block,
        3, // max retries
    ).await?;
    
    // 3. Validar cada pool y actualizar cache
    let mut validated_count = 0;
    for pool in &pools {
        let addr = pool.address();
        let is_valid = match pool {
            Pool::UniswapV2(_) => {
                fresh_states.v2_states.get(&addr)
                    .map(|state| {
                        state.0 > U256::zero() && 
                        state.1 > U256::zero()
                    })
                    .unwrap_or(false)
            }
            Pool::UniswapV3(_) => {
                fresh_states.v3_states.get(&addr)
                    .map(|state| {
                        state.sqrt_price_x96 > U256::zero() && 
                        state.liquidity > 0
                    })
                    .unwrap_or(false)
            }
            _ => false,
        };
        
        let liquidity = match pool {
            Pool::UniswapV2(_) => {
                fresh_states.v2_states.get(&addr)
                    .map(|s| s.0 + s.1)
            }
            Pool::UniswapV3(_) => {
                fresh_states.v3_states.get(&addr)
                    .map(|s| U256::from(s.liquidity))
            }
            _ => None,
        };
        
        cache.update(&addr, is_valid, liquidity, current_block);
        if is_valid {
            validated_count += 1;
        }
    }
    
    Ok(validated_count)
}

/// Fetch con retry y backoff exponencial
async fn fetch_with_retry(
    jit_fetcher: &JitStateFetcher,
    pool_metadata: &[PoolMetadata],
    current_block: u64,
    max_retries: usize,
) -> Result<FreshPoolStates> {
    let mut retries = 0;
    let mut backoff = Duration::from_millis(100);
    
    loop {
        match jit_fetcher.fetch_current_states(pool_metadata, current_block).await {
            Ok(states) => return Ok(states),
            Err(e) if retries < max_retries => {
                warn!("⚠️ JIT fetch failed (attempt {}/{}): {}", retries + 1, max_retries, e);
                sleep(backoff).await;
                backoff *= 2; // Backoff exponencial: 100ms → 200ms → 400ms
                retries += 1;
            }
            Err(e) => {
                error!("❌ JIT fetch failed after {} retries: {}", max_retries, e);
                return Err(anyhow!("JIT fetch failed after {} retries: {}", max_retries, e));
            }
        }
    }
}

