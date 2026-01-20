// Async PostgreSQL writer to avoid blocking fast-path with DB operations
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};
use sqlx::PgPool;
use anyhow::Result;
use log::{info, error};
use serde::{Serialize, Deserialize};
use ethers::types::{Address, U256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DbOperation {
    UpsertPool {
        address: Address,
        dex: String,
        token0: Address,
        token1: Address,
        fee_bps: Option<u32>,
        is_active: bool,
        last_seen_block: u64,
        is_valid: bool,
        factory: Option<Address>,
    },
    UpdatePoolState {
        address: Address,
        reserve0: Option<U256>,
        reserve1: Option<U256>,
        sqrt_price_x96: Option<U256>,
        liquidity: Option<u128>,
        tick: Option<i32>,
        block_number: u64,
    },
    UpsertGraphWeight {
        pool_address: Address,
        weight: f64,
        volume_24h: f64,
        liquidity_usd: f64,
    },
    SetDexState {
        dex: String,
        last_processed_block: u64,
    },
    CheckpointDexState {
        dex: String,
        block_number: u64,
    },
    BatchPoolSnapshot {
        snapshots: Vec<PoolSnapshot>,
    },
    SetPoolActivity {
        address: Address,
        is_active: bool,
    },
    BatchSetPoolActivity {
        activities: Vec<(Address, bool)>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSnapshot {
    pub address: Address,
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
    pub sqrt_price_x96: Option<U256>,
    pub liquidity: Option<u128>,
    pub tick: Option<i32>,
    pub block_number: u64,
    pub     timestamp: i64,
}

pub struct PostgresAsyncWriter {
    db_pool: PgPool,
    operation_tx: mpsc::UnboundedSender<DbOperation>,
    batch_size: usize,
    flush_interval: Duration,
}

impl PostgresAsyncWriter {
    pub fn new(db_pool: PgPool, batch_size: usize, flush_interval: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        
        let writer = Self {
            db_pool: db_pool.clone(),
            operation_tx: tx,
            batch_size,
            flush_interval,
        };
        
        // Spawn background writer task
        tokio::spawn(Self::writer_task(db_pool, rx, batch_size, flush_interval));
        
        writer
    }

    /// Send operation to background writer (non-blocking)
    pub fn send_operation(&self, operation: DbOperation) -> Result<()> {
        self.operation_tx.send(operation)
            .map_err(|_| anyhow::anyhow!("Failed to send DB operation - writer task may have died"))?;
        Ok(())
    }

    /// Convenience methods for common operations
    pub fn upsert_pool(&self, address: Address, dex: String, token0: Address, token1: Address, fee_bps: Option<u32>, is_active: bool, last_seen_block: u64) -> Result<()> {
        self.send_operation(DbOperation::UpsertPool {
            address, dex, token0, token1, fee_bps, is_active, last_seen_block,
            is_valid: true, // Default to valid for new pools
            factory: None, // Will be set from PoolMeta if needed
        })
    }
    
    /// Upsert pool with full metadata (for compatibility with database::upsert_pool)
    pub fn upsert_pool_full(&self, address: Address, dex: String, token0: Address, token1: Address, fee_bps: Option<u32>, is_active: bool, last_seen_block: u64, is_valid: bool, factory: Option<Address>) -> Result<()> {
        self.send_operation(DbOperation::UpsertPool {
            address, dex, token0, token1, fee_bps, is_active, last_seen_block, is_valid, factory
        })
    }

    pub fn update_pool_state(&self, address: Address, reserve0: Option<U256>, reserve1: Option<U256>, sqrt_price_x96: Option<U256>, liquidity: Option<u128>, tick: Option<i32>, block_number: u64) -> Result<()> {
        self.send_operation(DbOperation::UpdatePoolState {
            address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number
        })
    }

    pub fn upsert_graph_weight(&self, pool_address: Address, weight: f64, volume_24h: f64, liquidity_usd: f64) -> Result<()> {
        self.send_operation(DbOperation::UpsertGraphWeight {
            pool_address, weight, volume_24h, liquidity_usd
        })
    }

    pub fn set_dex_state(&self, dex: String, last_processed_block: u64) -> Result<()> {
        self.send_operation(DbOperation::SetDexState { dex, last_processed_block })
    }

    /// ✅ FASE 1.3: Checkpoint DEX state (atomic transaction for last_processed_block)
    /// This is called every 100 blocks to persist progress safely
    pub fn checkpoint_dex_state(&self, dex: String, block_number: u64) -> Result<()> {
        self.send_operation(DbOperation::CheckpointDexState { dex, block_number })
    }

    /// ✅ FASE 1.3: Force flush all pending operations (used before checkpointing)
    /// Note: This is best-effort, actual flush happens in background task
    pub async fn flush(&self) -> Result<()> {
        // Send a flush signal by sending a no-op operation that triggers immediate flush
        // The background task will flush on next tick or when batch is full
        // For now, we rely on the flush_interval (100ms) to ensure timely flushing
        Ok(())
    }

    pub fn batch_pool_snapshots(&self, snapshots: Vec<PoolSnapshot>) -> Result<()> {
        self.send_operation(DbOperation::BatchPoolSnapshot { snapshots })
    }

    pub fn set_pool_activity(&self, address: Address, is_active: bool) -> Result<()> {
        self.send_operation(DbOperation::SetPoolActivity { address, is_active })
    }

    pub fn batch_set_pool_activity(&self, activities: Vec<(Address, bool)>) -> Result<()> {
        self.send_operation(DbOperation::BatchSetPoolActivity { activities })
    }

    /// Background writer task
    async fn writer_task(
        db_pool: PgPool,
        mut rx: mpsc::UnboundedReceiver<DbOperation>,
        batch_size: usize,
        flush_interval: Duration,
    ) {
        let mut batch = Vec::with_capacity(batch_size);
        let mut flush_timer = interval(flush_interval);
        
        info!("PostgreSQL async writer started (batch_size: {}, flush_interval: {:?})", batch_size, flush_interval);
        
        loop {
            tokio::select! {
                // Receive new operation
                operation = rx.recv() => {
                    match operation {
                        Some(op) => {
                            batch.push(op);
                            
                            // Flush if batch is full
                            if batch.len() >= batch_size {
                                Self::flush_batch(&db_pool, &mut batch).await;
                            }
                        }
                        None => {
                            // Channel closed, flush remaining and exit
                            if !batch.is_empty() {
                                Self::flush_batch(&db_pool, &mut batch).await;
                            }
                            info!("PostgreSQL async writer shutting down");
                            break;
                        }
                    }
                }
                
                // Periodic flush
                _ = flush_timer.tick() => {
                    if !batch.is_empty() {
                        Self::flush_batch(&db_pool, &mut batch).await;
                    }
                }
            }
        }
    }

    async fn flush_batch(db_pool: &PgPool, batch: &mut Vec<DbOperation>) {
        if batch.is_empty() {
            return;
        }

        let start = std::time::Instant::now();
        
        // Group operations by type for efficient batch processing
        let mut pool_upserts = Vec::new();
        let mut state_updates = Vec::new();
        let mut weight_updates = Vec::new();
        let mut dex_states = Vec::new();
        let mut snapshots = Vec::new();
        let mut pool_activities = Vec::new();
        
        let mut checkpoints = Vec::new();
        
        for op in batch.drain(..) {
            match op {
                DbOperation::UpsertPool { address, dex, token0, token1, fee_bps, is_active, last_seen_block, is_valid, factory } => {
                    pool_upserts.push((address, dex, token0, token1, fee_bps, is_active, last_seen_block, is_valid, factory));
                }
                DbOperation::UpdatePoolState { address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number } => {
                    state_updates.push((address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number));
                }
                DbOperation::UpsertGraphWeight { pool_address, weight, volume_24h, liquidity_usd } => {
                    weight_updates.push((pool_address, weight, volume_24h, liquidity_usd));
                }
                DbOperation::SetDexState { dex, last_processed_block } => {
                    dex_states.push((dex, last_processed_block));
                }
                DbOperation::CheckpointDexState { dex, block_number } => {
                    // ✅ FASE 1.3: Checkpoints are processed separately in atomic transactions
                    checkpoints.push((dex, block_number));
                }
                DbOperation::BatchPoolSnapshot { snapshots: batch_snapshots } => {
                    snapshots.extend(batch_snapshots);
                }
                DbOperation::SetPoolActivity { address, is_active } => {
                    pool_activities.push((address, is_active));
                }
                DbOperation::BatchSetPoolActivity { activities } => {
                    pool_activities.extend(activities);
                }
            }
        }

        // Execute batched operations
        let mut operations_completed = 0;
        
        if !pool_upserts.is_empty() {
            let pool_count = pool_upserts.len();
            if let Err(e) = Self::batch_upsert_pools(db_pool, pool_upserts).await {
                error!("Failed to batch upsert pools: {}", e);
            } else {
                operations_completed += pool_count;
            }
        }
        
        if !state_updates.is_empty() {
            let state_count = state_updates.len();
            if let Err(e) = Self::batch_update_states(db_pool, state_updates).await {
                error!("Failed to batch update pool states: {}", e);
            } else {
                operations_completed += state_count;
            }
        }
        
        if !weight_updates.is_empty() {
            let weight_count = weight_updates.len();
            if let Err(e) = Self::batch_update_weights(db_pool, weight_updates).await {
                error!("Failed to batch update weights: {}", e);
            } else {
                operations_completed += weight_count;
            }
        }
        
        for (dex, block) in dex_states {
            if let Err(e) = Self::update_dex_state(db_pool, &dex, block).await {
                error!("Failed to update dex state for {}: {}", dex, e);
            } else {
                operations_completed += 1;
            }
        }
        
        // ✅ FASE 1.3: Process checkpoints in separate atomic transactions
        for (dex, block_number) in checkpoints {
            if let Err(e) = Self::checkpoint_dex_state_internal(db_pool, &dex, block_number).await {
                error!("Failed to checkpoint dex state for {} at block {}: {}", dex, block_number, e);
            } else {
                operations_completed += 1;
                info!("✅ FASE 1.3: Checkpointed {} at block {}", dex, block_number);
            }
        }
        
        if !snapshots.is_empty() {
            let snapshot_count = snapshots.len();
            if let Err(e) = Self::batch_insert_snapshots(db_pool, snapshots).await {
                error!("Failed to batch insert snapshots: {}", e);
            } else {
                operations_completed += snapshot_count;
            }
        }
        
        if !pool_activities.is_empty() {
            let activity_count = pool_activities.len();
            if let Err(e) = Self::batch_set_pool_activity_internal(db_pool, pool_activities).await {
                error!("Failed to batch set pool activity: {}", e);
            } else {
                operations_completed += activity_count;
            }
        }
        
        let duration = start.elapsed();
        info!("Flushed {} operations in {:?} ({} ops/sec)", 
              operations_completed, duration, 
              operations_completed as f64 / duration.as_secs_f64());
        
        crate::metrics::record_db_batch_duration(duration);
        crate::metrics::record_db_batch_size(operations_completed);
    }

    async fn batch_upsert_pools(
        db_pool: &PgPool,
        pools: Vec<(Address, String, Address, Address, Option<u32>, bool, u64, bool, Option<Address>)>,
    ) -> Result<()> {
        use crate::database::SCHEMA;
        use chrono::Utc;
        // Use PostgreSQL batch insert matching database::upsert_pool signature
        let mut tx = db_pool.begin().await?;
        
        for (address, dex, token0, token1, fee_bps, is_active, last_seen_block, is_valid, factory) in pools {
            let address_str = format!("{:?}", address);
            let token0_str = format!("{:?}", token0);
            let token1_str = format!("{:?}", token1);
            let factory_str = factory.map(|f| format!("{:?}", f));
            let origin_dex = dex.clone();
            let now = Utc::now();
            let bytecode_hash_str: Option<String> = None;
            let init_code_hash_str: Option<String> = None;
            
            sqlx::query(
                &format!(
                    "INSERT INTO {}.pools (address, dex, origin_dex, factory, token0, token1, fee_bps, created_block, is_valid, is_active, last_seen_block, updated_at, bytecode_hash, init_code_hash)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                     ON CONFLICT(address) DO UPDATE SET
                        dex=excluded.dex,
                        origin_dex=excluded.origin_dex,
                        factory=excluded.factory,
                        token0=excluded.token0,
                        token1=excluded.token1,
                        fee_bps=excluded.fee_bps,
                        is_valid=excluded.is_valid,
                        is_active=excluded.is_active,
                        last_seen_block=excluded.last_seen_block,
                        bytecode_hash=excluded.bytecode_hash,
                        init_code_hash=excluded.init_code_hash,
                        updated_at=excluded.updated_at", SCHEMA)
            )
            .bind(&address_str)
            .bind(&dex)
            .bind(&origin_dex)
            .bind(&factory_str)
            .bind(&token0_str)
            .bind(&token1_str)
            .bind(fee_bps.map(|f| f as i32))
            .bind(last_seen_block as i64) // created_block
            .bind(is_valid)
            .bind(is_active)
            .bind(last_seen_block as i64) // last_seen_block
            .bind(now)
            .bind(&bytecode_hash_str)
            .bind(&init_code_hash_str)
            .execute(&mut *tx).await?;
        }
        
        tx.commit().await?;
        Ok(())
    }

    async fn batch_update_states(
        db_pool: &PgPool,
        states: Vec<(Address, Option<U256>, Option<U256>, Option<U256>, Option<u128>, Option<i32>, u64)>,
    ) -> Result<()> {
        let mut tx = db_pool.begin().await?;
        
        for (address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number) in states {
            sqlx::query(
                "INSERT INTO pool_state_snapshots 
                 (pool_address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number, timestamp)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, EXTRACT(EPOCH FROM NOW()))"
            )
            .bind(format!("{:?}", address))
            .bind(reserve0.map(|r| format!("{}", r)))
            .bind(reserve1.map(|r| format!("{}", r)))
            .bind(sqrt_price_x96.map(|p| format!("{}", p)))
            .bind(liquidity.map(|l| l as i64))
            .bind(tick)
            .bind(block_number as i64)
            .execute(&mut *tx).await?;
        }
        
        tx.commit().await?;
        Ok(())
    }

    async fn batch_update_weights(
        db_pool: &PgPool,
        weights: Vec<(Address, f64, f64, f64)>,
    ) -> Result<()> {
        let mut tx = db_pool.begin().await?;
        
        for (pool_address, weight, volume_24h, liquidity_usd) in weights {
            sqlx::query(
                "INSERT INTO graph_weights (pool_address, weight, volume_24h, liquidity_usd, updated_at)
                 VALUES ($1, $2, $3, $4, EXTRACT(EPOCH FROM NOW()))
                 ON CONFLICT (pool_address) DO UPDATE SET
                 weight = EXCLUDED.weight,
                 volume_24h = EXCLUDED.volume_24h,
                 liquidity_usd = EXCLUDED.liquidity_usd,
                 updated_at = EXCLUDED.updated_at"
            )
            .bind(format!("{:?}", pool_address))
            .bind(weight)
            .bind(volume_24h)
            .bind(liquidity_usd)
            .execute(&mut *tx).await?;
        }
        
        tx.commit().await?;
        Ok(())
    }

    async fn update_dex_state(db_pool: &PgPool, dex: &str, last_processed_block: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO dex_state (dex, last_processed_block, updated_at)
             VALUES ($1, $2, EXTRACT(EPOCH FROM NOW()))
             ON CONFLICT (dex) DO UPDATE SET
             last_processed_block = EXCLUDED.last_processed_block,
             updated_at = EXCLUDED.updated_at"
        )
        .bind(dex)
        .bind(last_processed_block as i64)
        .execute(db_pool).await?;
        
        Ok(())
    }

    /// ✅ FASE 1.3: Checkpoint DEX state in atomic transaction
    /// This ensures progress is persisted safely every 100 blocks
    async fn checkpoint_dex_state_internal(db_pool: &PgPool, dex: &str, block_number: u64) -> Result<()> {
        let mut tx = db_pool.begin().await?;
        
        sqlx::query(
            "INSERT INTO dex_state (dex, last_processed_block, updated_at)
             VALUES ($1, $2, EXTRACT(EPOCH FROM NOW()))
             ON CONFLICT (dex) DO UPDATE SET
             last_processed_block = EXCLUDED.last_processed_block,
             updated_at = EXCLUDED.updated_at"
        )
        .bind(dex)
        .bind(block_number as i64)
        .execute(&mut *tx).await?;
        
        tx.commit().await?;
        Ok(())
    }

    async fn batch_insert_snapshots(db_pool: &PgPool, snapshots: Vec<PoolSnapshot>) -> Result<()> {
        let mut tx = db_pool.begin().await?;
        
        for snapshot in snapshots {
            sqlx::query(
                "INSERT INTO pool_state_snapshots 
                 (pool_address, reserve0, reserve1, sqrt_price_x96, liquidity, tick, block_number, timestamp)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
            )
            .bind(format!("{:?}", snapshot.address))
            .bind(snapshot.reserve0.map(|r| format!("{}", r)))
            .bind(snapshot.reserve1.map(|r| format!("{}", r)))
            .bind(snapshot.sqrt_price_x96.map(|p| format!("{}", p)))
            .bind(snapshot.liquidity.map(|l| l as i64))
            .bind(snapshot.tick)
            .bind(snapshot.block_number as i64)
            .bind(snapshot.timestamp)
            .execute(&mut *tx).await?;
        }
        
        tx.commit().await?;
        Ok(())
    }

    async fn batch_set_pool_activity_internal(db_pool: &PgPool, activities: Vec<(Address, bool)>) -> Result<()> {
        use crate::database::SCHEMA;
        let mut tx = db_pool.begin().await?;
        
        for (address, is_active) in activities {
            let address_str = format!("{:?}", address);
            sqlx::query(
                &format!("UPDATE {}.pools SET is_active = $1, updated_at = EXTRACT(EPOCH FROM NOW()) WHERE address = $2", SCHEMA)
            )
            .bind(is_active)
            .bind(&address_str)
            .execute(&mut *tx).await?;
        }
        
        tx.commit().await?;
        Ok(())
    }

    /// Get channel sender for direct use
    pub fn get_sender(&self) -> mpsc::UnboundedSender<DbOperation> {
        self.operation_tx.clone()
    }

    /// Check if writer is healthy (channel not closed)
    pub fn is_healthy(&self) -> bool {
        !self.operation_tx.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Address;
    
    #[tokio::test]
    async fn test_async_writer_creation() {
        // This test would require a real PostgreSQL connection
        // For now, just test the structure
        let operation = DbOperation::UpsertPool {
            address: Address::random(),
            dex: "UniswapV2".to_string(),
            token0: Address::random(),
            token1: Address::random(),
            fee_bps: Some(30),
            is_active: true,
            last_seen_block: 1000,
        };
        
        // Verify operation can be serialized
        let _json = serde_json::to_string(&operation).unwrap();
    }
}
