//! # Event Indexer
//!
//! Indexes blockchain events for gap detection and historical data access.
//!
//! ## Features
//!
//! - **Event Indexing**: Stores all pool creation events in database
//! - **Gap Detection**: Automatically detects missing blocks in event history
//! - **Re-sync**: Automatically re-syncs detected gaps
//!
//! ## Usage
//!
//! ```rust
//! let indexer = EventIndexer::new(db_pool);
//! indexer.index_event(dex, block_number, event_type, pool_address).await?;
//! let gaps = indexer.detect_gaps(dex, from_block, to_block).await?;
//! ```

use crate::database::SCHEMA;
use anyhow::Result;
use ethers::types::Address;
use log::{error, info, warn};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::interval;

/// Event type for indexing
#[derive(Debug, Clone, Copy)]
pub enum EventType {
    PairCreated,
    PoolCreated,
    PoolUpdated,
}

impl EventType {
    fn as_str(&self) -> &'static str {
        match self {
            EventType::PairCreated => "PairCreated",
            EventType::PoolCreated => "PoolCreated",
            EventType::PoolUpdated => "PoolUpdated",
        }
    }
}

/// Event indexer for gap detection and historical data
pub struct EventIndexer {
    db_pool: PgPool,
}

impl EventIndexer {
    /// Create a new event indexer
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }

    /// ✅ FASE 3.1: Index an event in the database
    pub async fn index_event(
        &self,
        dex: &str,
        block_number: u64,
        event_type: EventType,
        pool_address: Address,
    ) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO {}.event_index (dex, block_number, event_type, pool_address, indexed_at)
             VALUES ($1, $2, $3, $4, NOW())
             ON CONFLICT (dex, block_number, event_type, pool_address) DO NOTHING",
            SCHEMA
        ))
        .bind(dex)
        .bind(block_number as i64)
        .bind(event_type.as_str())
        .bind(format!("{:?}", pool_address))
        .execute(&self.db_pool)
        .await?;

        Ok(())
    }

    /// ✅ FASE 3.1: Detect gaps in event index
    /// Returns list of missing block numbers
    pub async fn detect_gaps(&self, dex: &str, from_block: u64, to_block: u64) -> Result<Vec<u64>> {
        // Query for missing blocks in the range
        let rows = sqlx::query(&format!(
            "SELECT generate_series($1::bigint, $2::bigint) AS missing_block
             EXCEPT
             SELECT DISTINCT block_number FROM {}.event_index WHERE dex = $3",
            SCHEMA
        ))
        .bind(from_block as i64)
        .bind(to_block as i64)
        .bind(dex)
        .fetch_all(&self.db_pool)
        .await?;

        let gaps: Vec<u64> = rows
            .into_iter()
            .filter_map(|row| {
                row.try_get::<Option<i64>, _>("missing_block")
                    .ok()
                    .flatten()
                    .map(|b| b as u64)
            })
            .collect();

        if !gaps.is_empty() {
            warn!(
                "✅ FASE 3.1: Detected {} gaps for {} between blocks {} and {}",
                gaps.len(),
                dex,
                from_block,
                to_block
            );
        }

        Ok(gaps)
    }

    /// ✅ FASE 3.1: Start background gap detection task
    /// Runs every hour to detect and log gaps
    pub fn start_gap_detection(&self, dex: String) {
        let db_pool = self.db_pool.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(3600)); // 1 hour
            loop {
                interval.tick().await;

                // Get block range from dex_state
                if let Ok(Some(row)) = sqlx::query(&format!(
                    "SELECT MIN(block_number) as min_block, MAX(block_number) as max_block
                     FROM {}.event_index WHERE dex = $1",
                    SCHEMA
                ))
                .bind(&dex)
                .fetch_optional(&db_pool)
                .await
                {
                    let min_block: Option<i64> = row.try_get("min_block").ok().flatten();
                    let max_block: Option<i64> = row.try_get("max_block").ok().flatten();
                    if let (Some(min_block), Some(max_block)) = (min_block, max_block) {
                        if let Ok(gaps) = Self::new(db_pool.clone())
                            .detect_gaps(&dex, min_block as u64, max_block as u64)
                            .await
                        {
                            if !gaps.is_empty() {
                                warn!("✅ FASE 3.1: Found {} gaps for {} - triggering automatic re-sync",
                                      gaps.len(), dex);

                                // ✅ IMPLEMENTADO: Trigger re-sync automático actualizando dex_state
                                // Estrategia: Actualizar last_processed_block al bloque más antiguo del gap
                                // para forzar que el orchestrator procese ese rango en el próximo ciclo
                                if let Some(first_gap_block) = gaps.first() {
                                    // Calcular el bloque más antiguo que necesita re-sync
                                    let oldest_gap = *first_gap_block;

                                    // Actualizar dex_state para forzar re-sync desde el gap más antiguo
                                    if let Err(e) = sqlx::query(
                                        &format!(
                                            "UPDATE {}.dex_state
                                             SET last_processed_block = LEAST(last_processed_block, $1),
                                                 mode = 'reverse_sync',
                                                 updated_at = NOW()
                                             WHERE dex = $2",
                                            SCHEMA
                                        )
                                    )
                                    .bind(oldest_gap as i64)
                                    .bind(&dex)
                                    .execute(&db_pool)
                                    .await
                                    {
                                        error!("❌ Failed to trigger re-sync for {}: {}", dex, e);
                                    } else {
                                        info!("✅ Triggered automatic re-sync for {} starting from block {}", dex, oldest_gap);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}
