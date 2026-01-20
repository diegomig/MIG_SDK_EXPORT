// Pool Validation Cache - Cache inteligente para validaciones de pools
// TTL dual: por bloques y por tiempo para mantener frescura

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use ethers::prelude::{Address, U256};

#[derive(Clone, Debug)]
pub struct CachedPoolValidation {
    pub is_valid: bool,
    pub liquidity: Option<U256>,
    pub cached_at_block: u64,
    pub last_checked: Instant,
    pub validation_count: u32, // Cuántas veces se ha validado
}

pub struct PoolValidationCache {
    cache: DashMap<Address, CachedPoolValidation>,
    metrics: CacheMetrics,
    ttl_blocks: u64,
    ttl_duration: Duration,
}

#[derive(Default)]
pub struct CacheMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    stale_evictions: AtomicU64,
    total_validations: AtomicU64,
}

impl PoolValidationCache {
    /// Crear nuevo cache con TTL personalizado
    pub fn new(ttl_blocks: u64, ttl_duration: Duration) -> Self {
        Self {
            cache: DashMap::new(),
            metrics: CacheMetrics::default(),
            ttl_blocks,
            ttl_duration,
        }
    }

    /// Crear cache con valores por defecto optimizados (5 minutos)
    pub fn new_default() -> Self {
        Self::new(
            25, // TTL: 25 bloques (~5 min en Ethereum)
            Duration::from_secs(300), // TTL: 5 minutos
        )
    }

    /// Obtener validación del cache (si está fresca)
    pub fn get(&self, addr: &Address, current_block: u64) -> Option<bool> {
        match self.cache.get(addr) {
            Some(cached) => {
                // Validar frescura por bloques Y tiempo
                let block_fresh = current_block.saturating_sub(cached.cached_at_block) < self.ttl_blocks;
                let time_fresh = cached.last_checked.elapsed() < self.ttl_duration;
                
                if block_fresh && time_fresh {
                    self.metrics.hits.fetch_add(1, Ordering::Relaxed);
                    Some(cached.is_valid)
                } else {
                    // Cache stale, remover
                    self.cache.remove(addr);
                    self.metrics.stale_evictions.fetch_add(1, Ordering::Relaxed);
                    self.metrics.misses.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
            None => {
                self.metrics.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Insertar validación en cache
    pub fn insert(&self, addr: Address, is_valid: bool, liquidity: Option<U256>, current_block: u64) {
        let cached = CachedPoolValidation {
            is_valid,
            liquidity,
            cached_at_block: current_block,
            last_checked: Instant::now(),
            validation_count: 1,
        };
        
        self.cache.insert(addr, cached);
        self.metrics.total_validations.fetch_add(1, Ordering::Relaxed);
    }

    /// Actualizar validación existente (incrementar contador)
    pub fn update(&self, addr: &Address, is_valid: bool, liquidity: Option<U256>, current_block: u64) {
        if let Some(mut cached) = self.cache.get_mut(addr) {
            cached.is_valid = is_valid;
            cached.liquidity = liquidity;
            cached.cached_at_block = current_block;
            cached.last_checked = Instant::now();
            cached.validation_count += 1;
        } else {
            self.insert(*addr, is_valid, liquidity, current_block);
        }
    }

    /// Invalidar pools stale proactivamente
    pub fn invalidate_stale(&self, current_block: u64) -> usize {
        let mut removed = 0;
        
        self.cache.retain(|_addr, cached| {
            let blocks_since_cache = current_block.saturating_sub(cached.cached_at_block);
            let is_stale = blocks_since_cache >= self.ttl_blocks 
                        || cached.last_checked.elapsed() >= self.ttl_duration;
            
            if is_stale {
                removed += 1;
                false
            } else {
                true
            }
        });
        
        self.metrics.stale_evictions.fetch_add(removed, Ordering::Relaxed);
        removed as usize
    }

    /// Obtener métricas del cache
    pub fn metrics(&self) -> CacheMetricsSnapshot {
        let hits = self.metrics.hits.load(Ordering::Relaxed);
        let misses = self.metrics.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        
        CacheMetricsSnapshot {
            hits,
            misses,
            hit_rate: if total > 0 { hits as f64 / total as f64 } else { 0.0 },
            cache_size: self.cache.len(),
            stale_evictions: self.metrics.stale_evictions.load(Ordering::Relaxed),
        }
    }

    /// Limpiar cache completamente (para testing)
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// Obtener tamaño del cache
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Verificar si el cache está vacío
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct CacheMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub cache_size: usize,
    pub stale_evictions: u64,
}

