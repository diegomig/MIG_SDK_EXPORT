// Deferred Discovery Queue - Cola de pools pendientes de validación
// Gestiona pools que no se pueden validar inmediatamente por limitaciones de RPC calls

use dashmap::DashMap;
use ethers::types::Address;
use std::sync::Arc;
use tracing::{debug, info, warn};
use crate::metrics;

use crate::pool_priority_classifier::ValidationPriority;

/// Validación pendiente de un pool
#[derive(Debug, Clone)]
pub struct PendingPoolValidation {
    pub pool_address: Address,
    pub discovered_at_block: u64,
    pub priority: ValidationPriority,
}

/// Cola diferida de validaciones de pools
pub struct DeferredDiscoveryQueue {
    pending_validations: Arc<DashMap<Address, PendingPoolValidation>>,
    max_pending: usize,
    max_age_blocks: u64,
}

impl DeferredDiscoveryQueue {
    /// Crea una nueva cola diferida
    pub fn new(max_pending: usize, max_age_blocks: u64) -> Self {
        Self {
            pending_validations: Arc::new(DashMap::new()),
            max_pending,
            max_age_blocks,
        }
    }

    /// Crea una cola con valores por defecto (100 pools, 100 bloques)
    pub fn new_default() -> Self {
        Self::new(100, 100)
    }

    /// Agrega un pool a la cola de validación pendiente
    pub fn add_pending(
        &self,
        pool: Address,
        block: u64,
        priority: ValidationPriority,
    ) -> Result<(), DeferredQueueError> {
        // Verificar límite máximo
        if self.pending_validations.len() >= self.max_pending {
            // Si la cola está llena, intentar descartar pools Low priority antiguos
            self.cleanup_old_low_priority(block);
            
            // Si todavía está llena, rechazar pools Low priority nuevos
            if priority == ValidationPriority::Low && self.pending_validations.len() >= self.max_pending {
                debug!(
                    "⚠️ [DeferredQueue] Queue full ({}), rejecting Low priority pool {}",
                    self.pending_validations.len(), pool
                );
                return Err(DeferredQueueError::QueueFull);
            }
        }

        let validation = PendingPoolValidation {
            pool_address: pool,
            discovered_at_block: block,
            priority,
        };

        self.pending_validations.insert(pool, validation);
        metrics::set_streaming_discovery_deferred_queue_size(self.pending_validations.len() as f64);
        debug!(
            "➕ [DeferredQueue] Added pool {} to queue (priority: {:?}, block: {})",
            pool, priority, block
        );

        Ok(())
    }

    /// Obtiene pools para validar en el bloque actual según el presupuesto de calls disponible
    /// Retorna pools ordenados por prioridad y antigüedad
    pub fn get_validations_for_block(
        &self,
        current_block: u64,
        max_calls: usize,
    ) -> Vec<Address> {
        if max_calls < 3 {
            // Necesitamos al menos 3 calls para validar un pool (bytecode, factory, token0)
            return Vec::new();
        }

        // Calcular cuántos pools podemos validar
        const CALLS_PER_POOL: usize = 3;
        let max_pools = max_calls / CALLS_PER_POOL;

        // Obtener todos los pools pendientes y ordenarlos
        let mut pending: Vec<PendingPoolValidation> = self
            .pending_validations
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        // Ordenar por prioridad (mayor primero) y antigüedad (más antiguo primero)
        pending.sort_by_key(|p| {
            let age = current_block.saturating_sub(p.discovered_at_block);
            let priority_score = p.priority.as_u8() as u64;
            // Prioridad más alta = score más alto, pero queremos orden descendente
            // Entonces usamos negativo para sort ascendente, luego revertimos
            (1000 - priority_score * 100, age)
        });

        // Tomar los top N pools
        let selected: Vec<Address> = pending
            .into_iter()
            .take(max_pools)
            .map(|p| p.pool_address)
            .collect();

        if !selected.is_empty() {
            info!(
                "📋 [DeferredQueue] Selected {} pools for validation (from {} pending, max_calls: {})",
                selected.len(),
                self.pending_validations.len(),
                max_calls
            );
        }

        selected
    }

    /// Remueve pools validados de la cola
    pub fn remove_validated(&self, pools: &[Address]) {
        for pool in pools {
            if let Some((_, validation)) = self.pending_validations.remove(pool) {
                let priority_str = match validation.priority {
                    ValidationPriority::Critical => "critical",
                    ValidationPriority::High => "high",
                    ValidationPriority::Medium => "medium",
                    ValidationPriority::Low => "low",
                };
                metrics::increment_streaming_discovery_deferred_pools_processed(priority_str, 1);
                debug!("✅ [DeferredQueue] Removed validated pool {} from queue", pool);
            }
        }
        metrics::set_streaming_discovery_deferred_queue_size(self.pending_validations.len() as f64);
    }

    /// Limpia pools antiguos (solo Low priority)
    fn cleanup_old_low_priority(&self, current_block: u64) {
        let mut to_remove = Vec::new();

        for entry in self.pending_validations.iter() {
            let validation = entry.value();
            let age = current_block.saturating_sub(validation.discovered_at_block);

            if validation.priority == ValidationPriority::Low && age > self.max_age_blocks {
                to_remove.push(validation.pool_address);
            }
        }

        for pool in to_remove {
            if self.pending_validations.remove(&pool).is_some() {
                warn!(
                    "🗑️ [DeferredQueue] Removed old Low priority pool {} (age: {} blocks)",
                    pool,
                    current_block.saturating_sub(
                        self.pending_validations
                            .get(&pool)
                            .map(|v| v.discovered_at_block)
                            .unwrap_or(0)
                    )
                );
            }
        }
    }

    /// Obtiene el tamaño actual de la cola
    pub fn len(&self) -> usize {
        let count = self.pending_validations.len();
        metrics::set_streaming_discovery_deferred_queue_size(count as f64);
        count
    }

    /// Verifica si la cola está vacía
    pub fn is_empty(&self) -> bool {
        self.pending_validations.is_empty()
    }

    /// Limpia todos los pools pendientes (útil para tests)
    #[cfg(test)]
    pub fn clear(&self) {
        self.pending_validations.clear();
    }
}

/// Errores de la cola diferida
#[derive(Debug, thiserror::Error)]
pub enum DeferredQueueError {
    #[error("Queue is full")]
    QueueFull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_add_and_get_pools() {
        let queue = DeferredDiscoveryQueue::new(10, 100);
        let pool1 = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();
        let pool2 = Address::from_str("0x2222222222222222222222222222222222222222").unwrap();

        queue
            .add_pending(pool1, 100, ValidationPriority::High)
            .unwrap();
        queue
            .add_pending(pool2, 100, ValidationPriority::Low)
            .unwrap();

        assert_eq!(queue.len(), 2);

        // Obtener pools para validar (9 calls = 3 pools máximo)
        let selected = queue.get_validations_for_block(101, 9);
        assert_eq!(selected.len(), 2); // Solo hay 2 pools
        assert_eq!(selected[0], pool1); // High priority primero
    }

    #[test]
    fn test_queue_full() {
        let queue = DeferredDiscoveryQueue::new(2, 100);
        let pool1 = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();
        let pool2 = Address::from_str("0x2222222222222222222222222222222222222222").unwrap();
        let pool3 = Address::from_str("0x3333333333333333333333333333333333333333").unwrap();

        queue
            .add_pending(pool1, 100, ValidationPriority::High)
            .unwrap();
        queue
            .add_pending(pool2, 100, ValidationPriority::High)
            .unwrap();

        // Low priority debería ser rechazado
        assert!(queue
            .add_pending(pool3, 100, ValidationPriority::Low)
            .is_err());
    }
}

