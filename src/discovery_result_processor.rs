// Discovery Result Processor - Procesa pools validados y los inserta en la DB
// Maneja la inserción asíncrona y actualización del grafo

use crate::metrics;
use anyhow::Result;
use ethers::prelude::Middleware;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::database::DbPool;
use crate::dex_adapter::PoolMeta;
use crate::graph_service::GraphService;
use crate::validator::ValidationResult;

/// Pool validado listo para insertar en la DB
#[derive(Debug)]
pub struct ValidatedPool {
    pub pool_meta: PoolMeta,
    pub validation_result: ValidationResult,
    pub discovered_at_block: u64,
}

/// Procesador de resultados de validación de pools
pub struct DiscoveryResultProcessor<M: Middleware> {
    db_pool: Arc<DbPool>,
    graph_service: Arc<GraphService<M>>,
}

impl<M: Middleware + 'static> DiscoveryResultProcessor<M> {
    /// Crea un nuevo procesador de resultados
    pub fn new(db_pool: Arc<DbPool>, graph_service: Arc<GraphService<M>>) -> Self {
        Self {
            db_pool,
            graph_service,
        }
    }

    /// Procesa pools validados: inserta en DB y actualiza grafo
    pub async fn process_validated_pools(
        &self,
        validated: Vec<ValidatedPool>,
    ) -> Result<ProcessedResults> {
        let start = Instant::now();
        let mut inserted = 0;
        let mut updated = 0;
        let mut invalid = 0;
        let mut errors = 0;

        for pool_data in validated {
            match &pool_data.validation_result {
                ValidationResult::Valid => {
                    match self
                        .insert_pool(&pool_data.pool_meta, pool_data.discovered_at_block)
                        .await
                    {
                        Ok(was_new) => {
                            if was_new {
                                inserted += 1;
                                metrics::increment_streaming_discovery_pools_inserted(1);
                                info!(
                                    "✅ [ResultProcessor] Inserted new pool: {} (DEX: {}, tokens: {:?}/{:?})",
                                    pool_data.pool_meta.address,
                                    pool_data.pool_meta.dex,
                                    pool_data.pool_meta.token0,
                                    pool_data.pool_meta.token1
                                );
                            } else {
                                updated += 1;
                                metrics::increment_streaming_discovery_pools_updated(1);
                                debug!(
                                    "🔄 [ResultProcessor] Updated existing pool: {}",
                                    pool_data.pool_meta.address
                                );
                            }
                        }
                        Err(e) => {
                            errors += 1;
                            warn!(
                                "❌ [ResultProcessor] Failed to insert pool {}: {}",
                                pool_data.pool_meta.address, e
                            );
                        }
                    }
                }
                ValidationResult::Invalid(reason) => {
                    invalid += 1;
                    debug!(
                        "⛔ [ResultProcessor] Pool {} invalid: {:?}",
                        pool_data.pool_meta.address, reason
                    );
                }
            }
        }

        // Actualizar grafo con los nuevos pools válidos
        if inserted > 0 {
            debug!(
                "🔄 [ResultProcessor] Updating graph with {} new pools",
                inserted
            );
            if let Err(e) = self.graph_service.calculate_and_update_all_weights().await {
                warn!("⚠️ [ResultProcessor] Failed to update graph weights: {}", e);
            }
        }

        let duration = start.elapsed();
        metrics::record_streaming_discovery_processing(duration);

        Ok(ProcessedResults {
            inserted,
            updated,
            invalid,
            errors,
        })
    }

    /// Inserta un pool en la DB (idempotente)
    async fn insert_pool(&self, pool: &PoolMeta, discovered_at_block: u64) -> Result<bool> {
        // Verificar si el pool ya existe
        let existing = sqlx::query("SELECT id FROM pools WHERE address = $1")
            .bind(format!("{:?}", pool.address))
            .fetch_optional(&*self.db_pool)
            .await?;

        if existing.is_some() {
            // Pool ya existe, actualizar metadata si es necesario
            sqlx::query(
                "UPDATE pools SET
                    dex = $2,
                    pool_type = $3,
                    fee = $4,
                    token0 = $5,
                    token1 = $6,
                    factory = $7,
                    updated_at = NOW()
                WHERE address = $1",
            )
            .bind(format!("{:?}", pool.address))
            .bind(&pool.dex)
            .bind(pool.pool_type.as_deref())
            .bind(pool.fee.map(|f| f as i32))
            .bind(format!("{:?}", pool.token0))
            .bind(format!("{:?}", pool.token1))
            .bind(pool.factory.map(|f| format!("{:?}", f)))
            .execute(&*self.db_pool)
            .await?;

            return Ok(false); // No era nuevo
        }

        // Insertar nuevo pool
        sqlx::query(
            "INSERT INTO pools (
                address, dex, pool_type, fee, token0, token1, factory,
                discovered_at_block, discovered_via_streaming, is_valid, is_active,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW())
            ON CONFLICT (address) DO UPDATE SET
                dex = EXCLUDED.dex,
                pool_type = EXCLUDED.pool_type,
                fee = EXCLUDED.fee,
                token0 = EXCLUDED.token0,
                token1 = EXCLUDED.token1,
                factory = EXCLUDED.factory,
                updated_at = NOW()",
        )
        .bind(format!("{:?}", pool.address))
        .bind(&pool.dex)
        .bind(pool.pool_type.as_deref())
        .bind(pool.fee.map(|f| f as i32))
        .bind(format!("{:?}", pool.token0))
        .bind(format!("{:?}", pool.token1))
        .bind(pool.factory.map(|f| format!("{:?}", f)))
        .bind(discovered_at_block as i64)
        .bind(true) // discovered_via_streaming = true
        .bind(true) // is_valid = true (ya fue validado)
        .bind(true) // is_active = true (activar por defecto)
        .execute(&*self.db_pool)
        .await?;

        Ok(true) // Era nuevo
    }
}

/// Resultados del procesamiento
#[derive(Debug, Clone)]
pub struct ProcessedResults {
    pub inserted: usize,
    pub updated: usize,
    pub invalid: usize,
    pub errors: usize,
}

impl ProcessedResults {
    /// Total de pools procesados
    pub fn total(&self) -> usize {
        self.inserted + self.updated + self.invalid + self.errors
    }

    /// Pools válidos procesados
    pub fn valid(&self) -> usize {
        self.inserted + self.updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processed_results() {
        let results = ProcessedResults {
            inserted: 5,
            updated: 2,
            invalid: 3,
            errors: 1,
        };

        assert_eq!(results.total(), 11);
        assert_eq!(results.valid(), 7);
    }
}
