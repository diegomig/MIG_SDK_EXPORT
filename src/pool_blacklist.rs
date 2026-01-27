// Pool Corruption Tracker - Trackea pools con estado corrupto para evitar fallbacks RPC excesivos
//
// MEJORAS IMPLEMENTADAS:
// 1. Rate-limit fallbacks RPC (máx N por pool por bloque)
// 2. Marcar pools como "corrupted" y excluirlos temporalmente
// 3. Auto-refresh de pools problemáticos

use dashmap::DashMap;
use ethers::types::Address;
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 🚀 GLOBAL SINGLETON: Tracker global de pools corruptos
/// Uso: `GLOBAL_CORRUPTION_TRACKER.try_register_fallback(pool)`
pub static GLOBAL_CORRUPTION_TRACKER: Lazy<PoolCorruptionTracker> = Lazy::new(|| {
    log::info!("🚀 PoolCorruptionTracker global initialized");
    PoolCorruptionTracker::new()
});

/// Tracks corrupted pools to prevent excessive RPC fallback attempts.
///
/// Identifies pools that consistently fail state fetches and temporarily excludes them
/// from refresh operations to reduce RPC load and improve system stability.
///
/// ## Features
///
/// - **Failure Tracking**: Tracks fallback counts per pool per block
/// - **Automatic Blacklisting**: Temporarily excludes pools with high failure rates
/// - **Auto-Recovery**: Automatically retries blacklisted pools after cooldown period
pub struct PoolCorruptionTracker {
    /// Pools marcados como corruptos: address -> (timestamp_marcado, block_cuando_se_marcó)
    corrupted_pools: DashMap<Address, (Instant, u64)>,

    /// Contador de fallbacks por pool por bloque: (address, block) -> count
    fallback_counts: DashMap<(Address, u64), u32>,

    /// Bloque actual para limpiar contadores viejos
    current_block: AtomicU64,

    /// Configuración
    config: CorruptionConfig,
}

/// Configuración del tracker
#[derive(Clone, Debug)]
pub struct CorruptionConfig {
    /// Máximo de fallbacks RPC por pool por bloque
    pub max_fallbacks_per_pool_per_block: u32,

    /// Tiempo que un pool permanece marcado como corrupto
    pub corruption_timeout: Duration,

    /// Bloques a esperar antes de re-intentar un pool corrupto
    pub blocks_before_retry: u64,
}

impl Default for CorruptionConfig {
    fn default() -> Self {
        Self {
            max_fallbacks_per_pool_per_block: 3,
            corruption_timeout: Duration::from_secs(86400), // ✅ SPRINT ESTABILIZACIÓN: 24 horas para pools que fallan en multicall
            blocks_before_retry: 10,
        }
    }
}

impl PoolCorruptionTracker {
    /// Crear nuevo tracker con configuración por defecto
    pub fn new() -> Self {
        Self::with_config(CorruptionConfig::default())
    }

    /// Crear tracker con configuración custom
    pub fn with_config(config: CorruptionConfig) -> Self {
        Self {
            corrupted_pools: DashMap::new(),
            fallback_counts: DashMap::new(),
            current_block: AtomicU64::new(0),
            config,
        }
    }

    /// Actualizar bloque actual y limpiar contadores viejos
    pub fn update_block(&self, block: u64) {
        let old_block = self.current_block.swap(block, Ordering::Relaxed);

        // Limpiar contadores de bloques viejos (más de 5 bloques atrás)
        if block > old_block + 5 {
            self.fallback_counts
                .retain(|(_, b), _| *b > block.saturating_sub(5));
        }

        // Limpiar pools corruptos que ya expiraron
        let now = Instant::now();
        self.corrupted_pools.retain(|_, (marked_at, marked_block)| {
            let time_ok = now.duration_since(*marked_at) < self.config.corruption_timeout;
            let block_ok = block < *marked_block + self.config.blocks_before_retry;
            time_ok || block_ok
        });
    }

    /// Verificar si un pool está marcado como corrupto
    /// ⚠️ CRITICAL FIX: Evitar deadlock usando get() y remove() separados
    pub fn is_corrupted(&self, pool: Address) -> bool {
        // Primero verificar si existe y obtener datos sin mantener el lock
        let entry_data = self.corrupted_pools.get(&pool).map(|entry| *entry);

        if let Some((marked_at, marked_block)) = entry_data {
            let current_block = self.current_block.load(Ordering::Relaxed);
            let now = Instant::now();

            // Pool sigue corrupto si no pasó el timeout NI los bloques de espera
            let time_expired = now.duration_since(marked_at) >= self.config.corruption_timeout;
            let blocks_passed = current_block >= marked_block + self.config.blocks_before_retry;

            if time_expired && blocks_passed {
                // Ya expiró, remover (ahora sin el lock de lectura activo)
                self.corrupted_pools.remove(&pool);
                false
            } else {
                true
            }
        } else {
            false
        }
    }

    /// Marcar un pool como corrupto
    pub fn mark_corrupted(&self, pool: Address, reason: &str) {
        let block = self.current_block.load(Ordering::Relaxed);
        self.corrupted_pools.insert(pool, (Instant::now(), block));
        log::warn!(
            "🔴 Pool {:?} marked as corrupted: {} (will retry after {}s or {} blocks)",
            pool,
            reason,
            self.config.corruption_timeout.as_secs(),
            self.config.blocks_before_retry
        );
        crate::metrics::increment_counter_named("pool_marked_corrupted".to_string());
    }

    /// Intentar registrar un fallback RPC para un pool
    /// Retorna true si el fallback está permitido, false si excede el límite
    pub fn try_register_fallback(&self, pool: Address) -> bool {
        let block = self.current_block.load(Ordering::Relaxed);
        let key = (pool, block);

        let mut count = self.fallback_counts.entry(key).or_insert(0);

        if *count >= self.config.max_fallbacks_per_pool_per_block {
            // Límite alcanzado, marcar como corrupto
            if !self.is_corrupted(pool) {
                self.mark_corrupted(pool, "exceeded fallback limit");
            }
            false
        } else {
            *count += 1;
            true
        }
    }

    /// Obtener estadísticas actuales
    pub fn stats(&self) -> CorruptionStats {
        CorruptionStats {
            corrupted_pools: self.corrupted_pools.len(),
            active_fallback_trackers: self.fallback_counts.len(),
            current_block: self.current_block.load(Ordering::Relaxed),
        }
    }

    /// Limpiar un pool específico (por ejemplo, después de refresh exitoso)
    pub fn clear_pool(&self, pool: Address) {
        self.corrupted_pools.remove(&pool);
        log::info!("🟢 Pool {:?} cleared from corruption tracker", pool);
    }

    /// ✅ SPRINT ESTABILIZACIÓN: Trackear pools que fallan en multicall
    /// Si un pool falla 3+ veces consecutivas, marcarlo como corrupto por 24 horas
    /// Retorna true si el pool fue blacklisteado (3+ fallos)
    pub fn try_register_multicall_failure(&self, pool: Address) -> bool {
        let block = self.current_block.load(Ordering::Relaxed);
        let key = (pool, block);

        let mut count = self.fallback_counts.entry(key).or_insert(0);
        *count += 1;

        // Si falla 3+ veces en el mismo bloque, marcarlo como corrupto
        if *count >= 3 {
            if !self.is_corrupted(pool) {
                self.mark_corrupted(pool, "multicall_failing_3_times");
            }
            return true;
        }

        false
    }
}

impl Default for PoolCorruptionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct CorruptionStats {
    pub corrupted_pools: usize,
    pub active_fallback_trackers: usize,
    pub current_block: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_limit() {
        let tracker = PoolCorruptionTracker::new();
        let pool = Address::random();
        tracker.update_block(100);

        // Primeros 3 fallbacks permitidos
        assert!(tracker.try_register_fallback(pool));
        assert!(tracker.try_register_fallback(pool));
        assert!(tracker.try_register_fallback(pool));

        // 4to fallback bloqueado
        assert!(!tracker.try_register_fallback(pool));

        // Pool debería estar marcado como corrupto
        assert!(tracker.is_corrupted(pool));
    }

    #[test]
    fn test_block_reset() {
        let tracker = PoolCorruptionTracker::new();
        let pool = Address::random();
        tracker.update_block(100);

        // Agotar límite
        for _ in 0..3 {
            tracker.try_register_fallback(pool);
        }
        assert!(!tracker.try_register_fallback(pool));

        // Nuevo bloque resetea contador (pero pool sigue corrupto)
        tracker.update_block(101);
        // Pool corrupto sigue bloqueado aunque sea nuevo bloque
        assert!(tracker.is_corrupted(pool));
    }
}
