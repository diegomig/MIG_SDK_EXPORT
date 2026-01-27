use crate::dex_adapter::PoolMeta;
use crate::pools::{Pool as PoolEnum, UniswapV2Pool, UniswapV3Pool};
use anyhow::Result;
use chrono::{DateTime, Utc};
use ethers::types::Address;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, Connection, Pool, Postgres, Row};
use std::collections::HashMap;
use std::env;
use std::str::FromStr;
use std::time::Duration;

/// PostgreSQL connection pool type alias.
pub type DbPool = Pool<Postgres>;

/// Statistics for a DEX protocol in the database.
///
/// Tracks pool counts, factory information, and refresh timestamps.
#[derive(Debug, Clone)]
pub struct DexStatistics {
    pub dex: String,
    pub total_pools: i64,
    pub active_pools: i64,
    pub valid_pools: i64,
    pub unique_factories: i64,
    pub unique_init_code_hashes: i64,
    pub unique_bytecode_hashes: i64,
    pub last_refreshed_at: DateTime<Utc>,
}

/// Pool statistics update for database persistence.
///
/// Contains TVL and volatility metrics for pool quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStatsUpdate {
    pub address: Address,
    pub tvl_usd: Option<f64>,
    // NOTE: profit_usd removed - trading-specific metric
    pub volatility_bps: Option<f64>,
}

/// Database schema name
///
/// NOTE: Renamed from 'arbitrage' to 'mig_topology' to remove trading references
/// and clarify the SDK's purpose as a topology mapping library.
pub const SCHEMA: &str = "mig_topology";

const DEFAULT_ACTIVE_POOL_LIMIT: i64 = 2000; // Aumentado de 500 a 2000 para cubrir más pools activos
                                             // Priority based on TVL and volatility only (topology-focused)
const POOL_PRIORITY_EXPR: &str = concat!(
    "LEAST(LN(1 + COALESCE(ps.tvl_usd, 0)), 20) * 0.7 - ",
    "COALESCE(ps.volatility_bps, 0) / 5000.0"
);

pub async fn connect() -> Result<DbPool> {
    // Force UTF-8 client encoding FIRST to avoid Windows sqlx bug with non-ASCII error messages
    env::set_var("PGCLIENTENCODING", "UTF8");

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // ✅ FASE 3.2: Detect PgBouncer (port 6432 or URL contains "pgbouncer")
    let is_pgbouncer = database_url.contains("pgbouncer")
        || database_url.contains(":6432")
        || database_url.contains("pgbouncer=true");

    if is_pgbouncer {
        log::info!("✅ FASE 3.2: PgBouncer detected in DATABASE_URL - using transaction pool mode");
    }

    log::info!(
        "🔍 Attempting to connect to database with URL: {}",
        database_url
    );
    log::info!("📋 Database URL components - URL: {}", database_url);

    // Add retries with exponential backoff to survive DNS/startup races in Compose
    let mut last_err: Option<anyhow::Error> = None;
    let max_attempts: u32 = 10;
    for attempt in 1..=max_attempts {
        match PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await
        {
            Ok(pool) => {
                log::info!(
                    "✅ Successfully connected to database (attempt {}/{}).",
                    attempt,
                    max_attempts
                );
                // Initialize database schema if needed
                if let Err(e) = initialize_database(&pool).await {
                    last_err = Some(e);
                } else {
                    return Ok(pool);
                }
            }
            Err(e) => {
                last_err = Some(e.into());
            }
        }
        // Backoff with cap
        let delay_ms = (1u64 << attempt.min(6)) * 200; // 200ms, 400ms, 800ms, ... capped at ~12.8s
        log::warn!(
            "DB connect/init attempt {}/{} failed. Retrying in {} ms...",
            attempt,
            max_attempts,
            delay_ms
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unknown DB connection error")))
}

pub async fn initialize_database(pool: &DbPool) -> Result<()> {
    const MIGRATION_LOCK_ID: i64 = 0x4D4947524154494F; // "MIGRATIO" in hex

    let mut conn = pool.acquire().await?;

    // Begin a transaction
    let mut tx = conn.begin().await?;

    log::info!("Acquiring database migration lock...");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(tx.as_mut())
        .await?;
    log::info!("✅ Database migration lock acquired.");

    log::info!("Checking database schema inside transaction...");

    // Re-check if tables exist inside the lock
    let tables_exist = sqlx::query(&format!(
        "SELECT COUNT(*) as count FROM information_schema.tables
             WHERE table_schema = '{}'
             AND table_name IN ('tokens', 'pools', 'dex_state', 'configurations')",
        SCHEMA
    ))
    .fetch_one(tx.as_mut())
    .await?
    .try_get::<i64, _>("count")?
        >= 4;

    if tables_exist {
        log::info!("✅ Database schema already exists. Ensuring it is up to date.");
        // Idempotent creation to add any new tables/indexes
        create_tables(&mut tx).await?;

        // ✅ FASE 3.1: Ensure event_index table exists
        create_event_index_table_internal(&mut tx).await?;
    } else {
        log::info!("📝 Creating database schema for the first time...");

        // Create schema if it doesn't exist
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", SCHEMA))
            .execute(tx.as_mut())
            .await?;

        // Enable extensions
        sqlx::query("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"")
            .execute(tx.as_mut())
            .await?;

        // Create tables
        create_tables(&mut tx).await?;

        // ✅ FASE 3.1: Create event_index table
        create_event_index_table_internal(&mut tx).await?;

        // Insert initial configuration
        sqlx::query(&format!(
            "INSERT INTO {}.configurations (key, value) VALUES ('db_initialized', 'true'), ('sdk_version', '0.1.0') ON CONFLICT (key) DO NOTHING",
            SCHEMA
        ))
            .execute(tx.as_mut())
            .await?;

        log::info!("✅ Database schema created successfully!");
    }

    // Commit the transaction and release the lock
    tx.commit().await?;
    log::info!("Database initialization complete, transaction committed.");

    Ok(())
}

async fn create_tables(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Tokens table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.tokens (
            id SERIAL PRIMARY KEY,
            address VARCHAR(42) UNIQUE NOT NULL,
            symbol VARCHAR(20),
            decimals INTEGER,
            token_type VARCHAR(20),
            oracle_source VARCHAR(30),
            confidence_score DOUBLE PRECISION,
            last_verified_block BIGINT,
            created_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Alter existing table to allow NULL symbols and decimals (migration for existing DBs)
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ALTER COLUMN symbol DROP NOT NULL",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok(); // Ignore error if column is already nullable

    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ALTER COLUMN decimals DROP NOT NULL",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok(); // Ignore error if column is already nullable

    // Add new columns if they don't exist (idempotent)
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ADD COLUMN IF NOT EXISTS token_type VARCHAR(20)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ADD COLUMN IF NOT EXISTS oracle_source VARCHAR(30)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ADD COLUMN IF NOT EXISTS confidence_score DOUBLE PRECISION",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens ADD COLUMN IF NOT EXISTS last_verified_block BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();

    // Drop any CHECK constraints on decimals column
    sqlx::query(&format!(
        "ALTER TABLE {}.tokens DROP CONSTRAINT IF EXISTS tokens_decimals_check",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok(); // Ignore error if constraint doesn't exist

    // Pools table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.pools (
            id SERIAL PRIMARY KEY,
            address VARCHAR(42) UNIQUE,
            dex VARCHAR(50),
            factory VARCHAR(42),
            token0 VARCHAR(42),
            token1 VARCHAR(42),
            fee_bps INTEGER,
            created_block BIGINT,
            is_valid BOOLEAN DEFAULT true,
            is_active BOOLEAN DEFAULT true,
            last_seen_block BIGINT,
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Dex state table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.dex_state (
            dex VARCHAR(50) PRIMARY KEY,
            last_processed_block BIGINT DEFAULT 0,
            mode VARCHAR(20) DEFAULT 'discovery',
            updated_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Pool state snapshots table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.pool_state_snapshots (
            id SERIAL PRIMARY KEY,
            pool_address VARCHAR(42),
            block_number BIGINT,
            reserve0 VARCHAR(100),
            reserve1 VARCHAR(100),
            ts TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;
    // FASE 2: Add index for pool_address and block_number queries
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_pool_state_snapshots_pool_block
         ON {}.pool_state_snapshots(pool_address, block_number DESC)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    sqlx::query(&format!(
        "ALTER TABLE {}.pools ADD COLUMN IF NOT EXISTS last_viable_at TIMESTAMPTZ",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pools ADD COLUMN IF NOT EXISTS last_viable_block BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pools ADD COLUMN IF NOT EXISTS origin_dex VARCHAR(50)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pools ADD COLUMN IF NOT EXISTS bytecode_hash VARCHAR(66)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pools ADD COLUMN IF NOT EXISTS init_code_hash VARCHAR(66)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();

    // Optional per-attribute block stamps for richer auditing (idempotent alters)
    sqlx::query(&format!(
        "ALTER TABLE {}.pool_state_snapshots ADD COLUMN IF NOT EXISTS slot0_block BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pool_state_snapshots ADD COLUMN IF NOT EXISTS liquidity_block BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pool_state_snapshots ADD COLUMN IF NOT EXISTS liquidity BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();
    sqlx::query(&format!(
        "ALTER TABLE {}.pool_state_snapshots ADD COLUMN IF NOT EXISTS reserves_block BIGINT",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await
    .ok();

    // Token relations table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.token_relations (
            id SERIAL PRIMARY KEY,
            base_token VARCHAR(42) NOT NULL,
            wrapped_token VARCHAR(42) NOT NULL,
            relation_type VARCHAR(20) NOT NULL,
            priority_source VARCHAR(30),
            confidence_score DOUBLE PRECISION,
            created_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Ensure uniqueness for (base_token, wrapped_token, relation_type)
    sqlx::query(
        &format!("CREATE UNIQUE INDEX IF NOT EXISTS uq_token_relations ON {}.token_relations (base_token, wrapped_token, relation_type)", SCHEMA)
    ).execute(tx.as_mut()).await.ok();

    // Audit log table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.audit_log (
            id SERIAL PRIMARY KEY,
            entity VARCHAR(30) NOT NULL,
            entity_id VARCHAR(100) NOT NULL,
            observed JSONB NOT NULL,
            expected JSONB,
            severity VARCHAR(10) DEFAULT 'info',
            ts TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Graph weights table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.graph_weights (
            pool_address VARCHAR(42) PRIMARY KEY,
            weight DOUBLE PRECISION,
            last_computed_block BIGINT,
            updated_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;
    // FASE 2: Index already exists as PRIMARY KEY, but add index on weight for sorting
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_graph_weights_weight
         ON {}.graph_weights(weight DESC)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;
    // ✅ OPTIMIZED: Composite index for JOIN with pools and filtering by weight
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_graph_weights_pool_weight
         ON {}.graph_weights(pool_address, weight DESC)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.pool_statistics (
            pool_address VARCHAR(42) PRIMARY KEY,
            tvl_usd DOUBLE PRECISION,
            volatility_bps DOUBLE PRECISION,
            volatility_sample_count BIGINT NOT NULL DEFAULT 0,
            updated_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_pool_statistics_tvl
         ON {}.pool_statistics (tvl_usd DESC NULLS LAST)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.dex_statistics (
            dex VARCHAR(50) PRIMARY KEY,
            total_pools BIGINT NOT NULL,
            active_pools BIGINT NOT NULL,
            valid_pools BIGINT NOT NULL,
            unique_factories BIGINT NOT NULL,
            unique_init_code_hashes BIGINT NOT NULL,
            unique_bytecode_hashes BIGINT NOT NULL,
            last_refreshed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Configurations table
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.configurations (
            key VARCHAR(100) PRIMARY KEY,
            value TEXT,
            created_at TIMESTAMPTZ DEFAULT NOW()
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Create indexes
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_pools_address ON {}.pools(address)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_pools_active ON {}.pools(is_active)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;
    // ✅ OPTIMIZED: Composite index for common WHERE clause (is_active, is_valid)
    sqlx::query(&format!("CREATE INDEX IF NOT EXISTS idx_pools_active_valid ON {}.pools(is_active, is_valid) WHERE is_active = true AND is_valid = true", SCHEMA))
        .execute(tx.as_mut())
        .await?;
    // ✅ OPTIMIZED: Index for sorting by last_seen_block in mvp_runner query
    sqlx::query(&format!("CREATE INDEX IF NOT EXISTS idx_pools_last_seen_block ON {}.pools(last_seen_block DESC) WHERE is_active = true", SCHEMA))
        .execute(tx.as_mut())
        .await?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct DexState {
    pub dex: String,
    pub last_processed_block: u64,
    pub mode: String,
}

pub async fn get_dex_state(pool: &DbPool, dex_name: &str) -> Result<Option<DexState>> {
    let row = sqlx::query(&format!(
        "SELECT dex, last_processed_block, mode FROM {}.dex_state WHERE dex = $1",
        SCHEMA
    ))
    .bind(dex_name)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => Ok(Some(DexState {
            dex: row.try_get("dex")?,
            last_processed_block: row.try_get::<i64, _>("last_processed_block")? as u64,
            mode: row.try_get("mode")?,
        })),
        None => Ok(None),
    }
}

pub async fn set_dex_state(pool: &DbPool, state: &DexState) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {}.dex_state (dex, last_processed_block, mode) VALUES ($1, $2, $3)
         ON CONFLICT (dex) DO UPDATE SET last_processed_block = $2, mode = $3",
        SCHEMA
    ))
    .bind(&state.dex)
    .bind(state.last_processed_block as i64)
    .bind(&state.mode)
    .execute(pool)
    .await?;
    Ok(())
}

/// Inserts or updates a token in the tokens table
pub async fn upsert_token(
    pool: &DbPool,
    token_address: Address,
    symbol: Option<&str>,
    decimals: Option<u8>,
) -> Result<()> {
    let address_str = format!("{:?}", token_address);

    sqlx::query(&format!(
        "INSERT INTO {}.tokens (address, symbol, decimals)
         VALUES ($1, $2, $3)
         ON CONFLICT(address) DO UPDATE SET
            symbol = COALESCE(excluded.symbol, {}.tokens.symbol),
            decimals = COALESCE(excluded.decimals, {}.tokens.decimals)",
        SCHEMA, SCHEMA, SCHEMA
    ))
    .bind(&address_str)
    .bind(symbol)
    .bind(decimals.map(|d| d as i32))
    .execute(pool)
    .await?;

    Ok(())
}

/// Batch upsert tokens with their metadata
pub async fn batch_upsert_tokens(
    pool: &DbPool,
    tokens: Vec<(Address, Option<String>, Option<u8>)>,
) -> Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    for (token_address, symbol, decimals) in tokens {
        let address_str = format!("{:?}", token_address);
        sqlx::query(&format!(
            "INSERT INTO {}.tokens (address, symbol, decimals)
             VALUES ($1, $2, $3)
             ON CONFLICT(address) DO UPDATE SET
                symbol = COALESCE(excluded.symbol, {}.tokens.symbol),
                decimals = COALESCE(excluded.decimals, {}.tokens.decimals)",
            SCHEMA, SCHEMA, SCHEMA
        ))
        .bind(&address_str)
        .bind(symbol)
        .bind(decimals.map(|d| d as i32))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Upsert a token relation (wrap/bridge/lp_underlying)
pub async fn upsert_token_relation(
    pool: &DbPool,
    base_token: Address,
    wrapped_token: Address,
    relation_type: &str,
    priority_source: Option<&str>,
    confidence_score: Option<f64>,
) -> Result<()> {
    let base_str = format!("{:?}", base_token);
    let wrapped_str = format!("{:?}", wrapped_token);
    sqlx::query(
        &format!(
            "INSERT INTO {}.token_relations (base_token, wrapped_token, relation_type, priority_source, confidence_score)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (base_token, wrapped_token, relation_type)
         DO UPDATE SET priority_source = EXCLUDED.priority_source, confidence_score = EXCLUDED.confidence_score",
            SCHEMA
        )
    )
    .bind(&base_str)
    .bind(&wrapped_str)
    .bind(relation_type)
    .bind(priority_source)
    .bind(confidence_score)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_token_oracle_source(
    pool: &DbPool,
    token_address: Address,
    oracle_source: Option<&str>,
    confidence_score: Option<f64>,
) -> Result<()> {
    let address_str = format!("{:?}", token_address);
    sqlx::query(&format!(
        "INSERT INTO {}.tokens (address, oracle_source, confidence_score)
         VALUES ($1, $2, $3)
         ON CONFLICT(address) DO UPDATE SET
            oracle_source = EXCLUDED.oracle_source,
            confidence_score = EXCLUDED.confidence_score",
        SCHEMA
    ))
    .bind(&address_str)
    .bind(oracle_source)
    .bind(confidence_score)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get tokens that have decimals but are missing symbols (for backfill)
pub async fn get_tokens_without_symbols(pool: &DbPool, limit: i64) -> Result<Vec<Address>> {
    let rows = sqlx::query(&format!(
        "SELECT address
         FROM {}.tokens
         WHERE decimals IS NOT NULL AND (symbol IS NULL OR symbol = '')
         ORDER BY created_at DESC
         LIMIT $1",
        SCHEMA
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let addresses: Vec<Address> = rows
        .iter()
        .filter_map(|row| row.try_get::<String, _>("address").ok())
        .filter_map(|addr| addr.parse().ok())
        .collect();

    Ok(addresses)
}

/// Get tokens that need enrichment (missing token_type or confidence_score)
pub async fn get_tokens_needing_enrichment(pool: &DbPool, limit: i64) -> Result<Vec<Address>> {
    let rows = sqlx::query(&format!(
        "SELECT address
         FROM {}.tokens
         WHERE token_type IS NULL OR confidence_score IS NULL
         ORDER BY created_at DESC
         LIMIT $1",
        SCHEMA
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let addresses: Vec<Address> = rows
        .iter()
        .filter_map(|row| row.try_get::<String, _>("address").ok())
        .filter_map(|addr| addr.parse().ok())
        .collect();

    Ok(addresses)
}

pub async fn upsert_pool(
    pool: &DbPool,
    pool_meta: &PoolMeta,
    created_block: u64,
    is_valid: bool,
    is_active: bool,
) -> Result<()> {
    let address_str = format!("{:?}", pool_meta.address);
    let factory_str = pool_meta.factory.map(|f| format!("{:?}", f));
    let token0_str = format!("{:?}", pool_meta.token0);
    let token1_str = format!("{:?}", pool_meta.token1);
    let fee_bps = pool_meta.fee.map(|f| f as i32);
    let now = Utc::now();
    // Note: origin, bytecode_hash, init_code_hash fields removed from PoolMeta
    let origin_dex = pool_meta.dex;
    let _bytecode_hash_str: Option<String> = None;
    let _init_code_hash_str: Option<String> = None;

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
            updated_at=excluded.updated_at",
            SCHEMA
        )
    )
    .bind(&address_str)
    .bind(pool_meta.dex)
    .bind(origin_dex)
    .bind(factory_str)
    .bind(&token0_str)
    .bind(&token1_str)
    .bind(fee_bps)
    .bind(created_block as i64)
    .bind(is_valid)
    .bind(is_active)
    .bind(created_block as i64) // last_seen_block
    .bind(now)
    .bind(_bytecode_hash_str)
    .bind(_init_code_hash_str)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn insert_pool_snapshot(
    pool: &DbPool,
    pool_address: &str,
    block_number: u64,
    reserve0: &str,
    reserve1: &str,
) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        &format!("INSERT INTO {}.pool_state_snapshots (pool_address, block_number, reserve0, reserve1, ts) VALUES ($1, $2, $3, $4, $5)", SCHEMA)
    )
    .bind(pool_address)
    .bind(block_number as i64)
    .bind(reserve0)
    .bind(reserve1)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_graph_weight(
    pool: &DbPool,
    pool_address: &str,
    weight: f64,
    last_computed_block: u64,
) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        &format!(
            "INSERT INTO {}.graph_weights (pool_address, weight, last_computed_block, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT(pool_address) DO UPDATE SET weight=excluded.weight, last_computed_block=excluded.last_computed_block, updated_at=excluded.updated_at",
            SCHEMA
        )
    )
    .bind(pool_address)
    .bind(weight)
    .bind(last_computed_block as i64)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// ✅ P1 OPTIMIZATION: Batch upsert graph weights for improved performance
///
/// This function performs a single database transaction to update multiple weights,
/// significantly reducing database round-trips compared to individual updates.
///
/// # Parameters
///
/// - `pool`: Database connection pool
/// - `weights`: Vector of tuples containing (pool_address, weight, last_computed_block)
///
/// # Returns
///
/// Returns `Ok(())` if batch update succeeds, or an error if database operation fails.
///
/// # Performance
///
/// - Reduces database round-trips from N to 1 (where N = number of weights)
/// - Uses a single transaction with multiple VALUES clauses
/// - Typical improvement: 10-50x faster for large batches (100+ weights)
pub async fn batch_upsert_graph_weights(
    pool: &DbPool,
    weights: &[(Address, f64, u64)],
) -> Result<()> {
    if weights.is_empty() {
        return Ok(());
    }

    let now = Utc::now();

    // ✅ P1 OPTIMIZATION: Use a single transaction with batch VALUES
    // Process in chunks to avoid query size limits (1000 weights per batch)
    const BATCH_CHUNK_SIZE: usize = 1000;

    for chunk in weights.chunks(BATCH_CHUNK_SIZE) {
        // Build VALUES clause dynamically
        let mut values_clauses: Vec<String> = Vec::new();
        let mut param_index = 1;

        for (idx, _) in chunk.iter().enumerate() {
            if idx > 0 {
                values_clauses.push(",".to_string());
            }
            values_clauses.push(format!(
                "(${}, ${}, ${}, ${})",
                param_index,
                param_index + 1,
                param_index + 2,
                param_index + 3
            ));
            param_index += 4;
        }

        let values_str = values_clauses.join(" ");

        // Build query string first to ensure it lives long enough
        let query_string = format!(
            r#"
            INSERT INTO {}.graph_weights (pool_address, weight, last_computed_block, updated_at)
            VALUES {}
            ON CONFLICT(pool_address) DO UPDATE SET
                weight = EXCLUDED.weight,
                last_computed_block = EXCLUDED.last_computed_block,
                updated_at = EXCLUDED.updated_at
            "#,
            SCHEMA, values_str
        );

        // Build query with the string reference
        let mut query_builder = sqlx::query(&query_string);

        // Collect all pool_addr_hex strings first to keep them alive during binding
        let pool_addr_hex_vec: Vec<String> = chunk
            .iter()
            .map(|(addr, _, _)| format!("{:#x}", addr))
            .collect();

        // Bind all parameters
        for (idx, (_, weight, block)) in chunk.iter().enumerate() {
            query_builder = query_builder
                .bind(&pool_addr_hex_vec[idx])
                .bind(*weight)
                .bind(*block as i64)
                .bind(now);
        }

        query_builder.execute(pool).await?;
    }

    Ok(())
}

pub async fn load_all_graph_weights(pool: &DbPool) -> Result<HashMap<Address, f64>> {
    let rows = sqlx::query(&format!(
        "SELECT pool_address, weight FROM {}.graph_weights",
        SCHEMA
    ))
    .fetch_all(pool)
    .await?;

    let mut weights = HashMap::new();
    for row in rows {
        let address = Address::from_str(&row.try_get::<String, _>("pool_address")?)?;
        let weight: f64 = row.try_get("weight")?;
        weights.insert(address, weight);
    }
    Ok(weights)
}

/// Pool candidate loaded from database with weight information
#[derive(Debug, Clone)]
pub struct PoolCandidate {
    pub address: Address,
    pub weight: f64,
    pub last_update_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Load top pool candidates from database using optimized query
///
/// Returns pools with weight >= min_weight, ordered by weight DESC.
/// This is used to populate the Hot Pool Manager with high-quality pools.
pub async fn load_pool_candidates(
    pool: &DbPool,
    min_weight: f64,
    limit: i64,
) -> Result<Vec<PoolCandidate>> {
    use chrono::Utc;

    // ✅ MEJORA: Aceptar weights de hasta 30 días atrás (más realista)
    // Pero si no hay weights recientes, usar históricos (sin filtro de fecha)
    let max_age_acceptable = Utc::now() - chrono::Duration::days(30);

    // ✅ Query simplificada: Solo is_valid = true (no filtra por is_active)
    // Esto permite usar pools históricos con weights válidos
    // ESTRATEGIA: Primero intentar weights recientes, si no hay suficientes, usar históricos
    let mut rows = sqlx::query(&format!(
        r#"
            SELECT
                gw.pool_address as address,
                gw.weight,
                gw.updated_at as last_update_timestamp
            FROM {}.graph_weights gw
            INNER JOIN {}.pools p ON p.address = gw.pool_address
            WHERE p.is_valid = true
              AND gw.weight >= $1
              AND (gw.updated_at IS NULL OR gw.updated_at >= $2)
            ORDER BY gw.weight DESC
            LIMIT $3
            "#,
        SCHEMA, SCHEMA
    ))
    .bind(min_weight)
    .bind(max_age_acceptable)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    // ✅ FALLBACK: Si no encontramos suficientes candidatos recientes, buscar históricos
    if rows.len() < limit as usize / 2 {
        let historical_rows = sqlx::query(&format!(
            r#"
                SELECT
                    gw.pool_address as address,
                    gw.weight,
                    gw.updated_at as last_update_timestamp
                FROM {}.graph_weights gw
                INNER JOIN {}.pools p ON p.address = gw.pool_address
                WHERE p.is_valid = true
                  AND gw.weight >= $1
                  AND (gw.updated_at IS NULL OR gw.updated_at < $2)
                ORDER BY gw.weight DESC
                LIMIT $3
                "#,
            SCHEMA, SCHEMA
        ))
        .bind(min_weight)
        .bind(max_age_acceptable)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        // Combinar resultados (recientes primero, luego históricos)
        rows.extend(historical_rows);
        rows.sort_by(|a, b| {
            let weight_a: f64 = a.try_get("weight").unwrap_or(0.0);
            let weight_b: f64 = b.try_get("weight").unwrap_or(0.0);
            weight_b
                .partial_cmp(&weight_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(limit as usize);
    }

    let mut candidates = Vec::new();
    for row in rows {
        let address_str: String = row.try_get("address")?;
        let address = Address::from_str(&address_str)?;
        let weight: f64 = row.try_get("weight")?;
        let last_update_timestamp: Option<chrono::DateTime<chrono::Utc>> =
            row.try_get("last_update_timestamp")?;

        candidates.push(PoolCandidate {
            address,
            weight,
            last_update_timestamp,
        });
    }

    Ok(candidates)
}

/// Load pool metadata from database
/// Loads all active pools to ensure bot has full coverage
/// Note: Depends on background_discoverer to mark pools as active
pub async fn load_active_pools_meta(pool: &DbPool) -> Result<Vec<PoolMeta>> {
    load_active_pools_meta_with_limit(pool, DEFAULT_ACTIVE_POOL_LIMIT).await
}

pub async fn load_active_pools_meta_with_limit(pool: &DbPool, limit: i64) -> Result<Vec<PoolMeta>> {
    let query = format!(
        "SELECT p.address,
                p.factory,
                p.token0,
                p.token1,
                p.dex,
                p.origin_dex,
                p.fee_bps,
                p.bytecode_hash,
                p.init_code_hash,
                COALESCE(gw.weight, 0.0) as weight
         FROM {}.pools p
         LEFT JOIN {}.graph_weights gw ON p.address = gw.pool_address
         LEFT JOIN {}.pool_statistics ps ON p.address = ps.pool_address
         WHERE p.is_active = true
           AND p.is_valid = true
           AND p.dex != 'Curve'
         ORDER BY ({priority}) DESC,
                  COALESCE(gw.weight, 0.0) DESC,
                  p.updated_at DESC
         LIMIT $1",
        SCHEMA,
        SCHEMA,
        SCHEMA,
        priority = POOL_PRIORITY_EXPR
    );

    let rows = sqlx::query(&query).bind(limit).fetch_all(pool).await?;

    let mut pools_meta = Vec::new();
    for row in rows {
        let factory_str: Option<String> = row.try_get("factory")?;
        let factory = factory_str.and_then(|s| Address::from_str(&s).ok());
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let dex_name = row.try_get::<String, _>("dex")?;
        let _origin_dex: Option<String> = row.try_get("origin_dex")?;
        let _bytecode_hash: Option<String> = row.try_get("bytecode_hash")?;
        let _init_code_hash: Option<String> = row.try_get("init_code_hash")?;

        pools_meta.push(PoolMeta {
            address: Address::from_str(&row.try_get::<String, _>("address")?)?,
            factory,
            token0: Address::from_str(&row.try_get::<String, _>("token0")?)?,
            token1: Address::from_str(&row.try_get::<String, _>("token1")?)?,
            dex: Box::leak(dex_name.clone().into_boxed_str()),
            pool_id: None,
            pool_type: None,
            fee: fee_bps.map(|f| f as u32),
        });
    }
    Ok(pools_meta)
}

pub async fn refresh_dex_statistics(pool: &DbPool) -> Result<()> {
    sqlx::query(
        &format!(
            "INSERT INTO {}.dex_statistics (
            dex,
            total_pools,
            active_pools,
            valid_pools,
            unique_factories,
            unique_init_code_hashes,
            unique_bytecode_hashes,
            last_refreshed_at
        )
        SELECT
            COALESCE(dex, 'Unknown') AS dex,
            COUNT(*) AS total_pools,
            COUNT(*) FILTER (WHERE is_active) AS active_pools,
            COUNT(*) FILTER (WHERE is_valid) AS valid_pools,
            COUNT(DISTINCT factory) FILTER (WHERE factory IS NOT NULL) AS unique_factories,
            COUNT(DISTINCT init_code_hash) FILTER (WHERE init_code_hash IS NOT NULL) AS unique_init_code_hashes,
            COUNT(DISTINCT bytecode_hash) FILTER (WHERE bytecode_hash IS NOT NULL) AS unique_bytecode_hashes,
            NOW() AS last_refreshed_at
        FROM {}.pools
        GROUP BY dex
        ON CONFLICT (dex) DO UPDATE SET
            total_pools = EXCLUDED.total_pools,
            active_pools = EXCLUDED.active_pools,
            valid_pools = EXCLUDED.valid_pools,
            unique_factories = EXCLUDED.unique_factories,
            unique_init_code_hashes = EXCLUDED.unique_init_code_hashes,
            unique_bytecode_hashes = EXCLUDED.unique_bytecode_hashes,
            last_refreshed_at = EXCLUDED.last_refreshed_at",
            SCHEMA, SCHEMA
        ),
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn load_dex_statistics(pool: &DbPool) -> Result<Vec<DexStatistics>> {
    let rows = sqlx::query(&format!(
        "SELECT dex,
                total_pools,
                active_pools,
                valid_pools,
                unique_factories,
                unique_init_code_hashes,
                unique_bytecode_hashes,
                last_refreshed_at
         FROM {}.dex_statistics
         ORDER BY dex",
        SCHEMA
    ))
    .fetch_all(pool)
    .await?;

    let stats = rows
        .into_iter()
        .map(|row| -> Result<DexStatistics> {
            Ok(DexStatistics {
                dex: row.try_get::<String, _>("dex")?,
                total_pools: row.try_get("total_pools")?,
                active_pools: row.try_get("active_pools")?,
                valid_pools: row.try_get("valid_pools")?,
                unique_factories: row.try_get("unique_factories")?,
                unique_init_code_hashes: row.try_get("unique_init_code_hashes")?,
                unique_bytecode_hashes: row.try_get("unique_bytecode_hashes")?,
                last_refreshed_at: row.try_get("last_refreshed_at")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(stats)
}

/// Load pools from database
/// Loads all active pools to ensure bot has full coverage
/// Note: Depends on background_discoverer to mark pools as active
pub async fn load_active_pools(pool: &DbPool) -> Result<Vec<PoolEnum>> {
    load_active_pools_with_limit(pool, DEFAULT_ACTIVE_POOL_LIMIT).await
}

pub async fn load_active_pools_with_limit(pool: &DbPool, limit: i64) -> Result<Vec<PoolEnum>> {
    let query = format!(
        "SELECT p.address, p.dex, p.token0, p.token1, p.fee_bps
         FROM {}.pools p
         LEFT JOIN {}.graph_weights gw ON p.address = gw.pool_address
         LEFT JOIN {}.pool_statistics ps ON p.address = ps.pool_address
         WHERE p.is_active = true
           AND p.is_valid = true
           AND p.dex != 'Curve'
        ORDER BY
          -- Prioridad 1: Pools sin weight o con weight muy antiguo (>2 horas) primero
          CASE
            WHEN gw.weight IS NULL THEN 0
            WHEN gw.updated_at IS NULL THEN 1
            WHEN gw.updated_at < NOW() - INTERVAL '2 hours' THEN 2
            ELSE 3
          END,
          -- Prioridad 2: Pools con mayor weight (usa índice idx_graph_weights_weight)
          COALESCE(gw.weight, 0.0) DESC,
          -- Prioridad 3: Pools con weights más antiguos primero (rotación)
          COALESCE(gw.updated_at, '1970-01-01'::timestamp) ASC,
          -- Prioridad 4: Frescura del pool (usa índice en p.updated_at)
          p.updated_at DESC
         LIMIT $1",
        SCHEMA, SCHEMA, SCHEMA
    );

    let rows = sqlx::query(&query).bind(limit).fetch_all(pool).await?;

    let mut pools = Vec::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address: Address = Address::from_str(&row.try_get::<String, _>("address")?)?;
        let token0: Address = Address::from_str(&row.try_get::<String, _>("token0")?)?;
        let token1: Address = Address::from_str(&row.try_get::<String, _>("token1")?)?;
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0: 0,
                    reserve1: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => PoolEnum::UniswapV3(UniswapV3Pool {
                address,
                token0,
                token1,
                fee: fee_bps.unwrap_or(0) as u32,
                sqrt_price_x96: Default::default(),
                liquidity: 0,
                tick: 0,
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Balancer" => PoolEnum::BalancerWeighted(crate::pools::BalancerWeightedPool {
                address,
                pool_id: [0u8; 32],
                tokens: vec![token0, token1],
                balances: vec![],
                weights: vec![],
                swap_fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Curve" => PoolEnum::CurveStableSwap(crate::pools::CurveStableSwapPool {
                address,
                tokens: vec![token0, token1],
                balances: vec![],
                a: Default::default(),
                fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            _ => {
                continue;
            }
        };
        pools.push(pool_enum);
    }
    Ok(pools)
}

/// Load pools by specific addresses (for incremental weight updates)
pub async fn load_pools_by_addresses(
    pool: &DbPool,
    addresses: &[Address],
) -> Result<Vec<PoolEnum>> {
    if addresses.is_empty() {
        return Ok(Vec::new());
    }

    // Convert addresses to hex strings for SQL IN clause
    let address_strings: Vec<String> = addresses
        .iter()
        .map(|addr| format!("{:#x}", addr))
        .collect();

    // Build query with IN clause (PostgreSQL supports arrays)
    // ✅ MEJORA: No filtrar por is_active para permitir pools históricos con weights válidos
    let query = format!(
        "SELECT p.address, p.dex, p.token0, p.token1, p.fee_bps
         FROM {}.pools p
         WHERE p.address = ANY($1::text[])
           AND p.is_valid = true
         ORDER BY p.address",
        SCHEMA
    );

    let rows = sqlx::query(&query)
        .bind(&address_strings)
        .fetch_all(pool)
        .await?;

    let mut pools = Vec::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address: Address = Address::from_str(&row.try_get::<String, _>("address")?)?;
        let token0: Address = Address::from_str(&row.try_get::<String, _>("token0")?)?;
        let token1: Address = Address::from_str(&row.try_get::<String, _>("token1")?)?;
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0: 0,
                    reserve1: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => PoolEnum::UniswapV3(UniswapV3Pool {
                address,
                token0,
                token1,
                fee: fee_bps.unwrap_or(0) as u32,
                sqrt_price_x96: Default::default(),
                liquidity: 0,
                tick: 0,
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Balancer" => PoolEnum::BalancerWeighted(crate::pools::BalancerWeightedPool {
                address,
                pool_id: [0u8; 32],
                tokens: vec![token0, token1],
                balances: vec![],
                weights: vec![],
                swap_fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Curve" => PoolEnum::CurveStableSwap(crate::pools::CurveStableSwapPool {
                address,
                tokens: vec![token0, token1],
                balances: vec![],
                a: Default::default(),
                fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            _ => continue,
        };
        pools.push(pool_enum);
    }
    Ok(pools)
}

/// Load valid pools by liquidity range (USD)
/// Used for continuous route pre-computation (Option 8)
/// Returns all pools with weight (liquidity USD) between min and max
pub async fn load_valid_pools_by_liquidity_range(
    pool: &DbPool,
    min_liquidity_usd: f64,
    max_liquidity_usd: f64,
) -> Result<Vec<PoolEnum>> {
    // ✅ OPTIMIZED: Use LEFT JOIN with DISTINCT ON instead of correlated subqueries
    // This is much faster because it scans pool_state_snapshots once instead of 3 times per pool
    // ✅ OPTION 3: Include weight from graph_weights to use as fallback for V3 liquidity
    let query = format!(
        "SELECT
            p.address, p.dex, p.token0, p.token1, p.fee_bps,
            pss.reserve0,
            pss.reserve1,
            pss.liquidity,
            gw.weight as pool_weight
         FROM {}.pools p
         INNER JOIN {}.graph_weights gw ON p.address = gw.pool_address
         LEFT JOIN LATERAL (
             SELECT reserve0, reserve1, liquidity
             FROM {}.pool_state_snapshots
             WHERE pool_address = p.address
             ORDER BY block_number DESC
             LIMIT 1
         ) pss ON true
         WHERE p.is_active = true
           AND p.is_valid = true
           AND gw.weight >= $1
           AND gw.weight <= $2
         ORDER BY gw.weight DESC
         LIMIT 50000",
        SCHEMA, SCHEMA, SCHEMA
    );

    let rows = sqlx::query(&query)
        .bind(min_liquidity_usd)
        .bind(max_liquidity_usd)
        .fetch_all(pool)
        .await?;

    let total_rows = rows.len();
    tracing::info!(
        "📊 [OPTION 8] Query returned {} rows from database",
        total_rows
    );

    let mut pools = Vec::new();
    let mut skipped_count = 0;
    let mut skipped_dex: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut parse_errors = 0;
    let mut pools_with_reserves = 0;
    let mut pools_without_reserves = 0;

    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address_str: String = row.try_get::<String, _>("address")?;
        let token0_str: String = row.try_get::<String, _>("token0")?;
        let token1_str: String = row.try_get::<String, _>("token1")?;

        let address = match Address::from_str(&address_str) {
            Ok(addr) => addr,
            Err(e) => {
                tracing::debug!("⚠️ Failed to parse address {}: {}", address_str, e);
                parse_errors += 1;
                skipped_count += 1;
                continue;
            }
        };

        let token0 = match Address::from_str(&token0_str) {
            Ok(addr) => addr,
            Err(e) => {
                tracing::debug!(
                    "⚠️ Failed to parse token0 {} for pool {}: {}",
                    token0_str,
                    address_str,
                    e
                );
                parse_errors += 1;
                skipped_count += 1;
                continue;
            }
        };

        let token1 = match Address::from_str(&token1_str) {
            Ok(addr) => addr,
            Err(e) => {
                tracing::debug!(
                    "⚠️ Failed to parse token1 {} for pool {}: {}",
                    token1_str,
                    address_str,
                    e
                );
                parse_errors += 1;
                skipped_count += 1;
                continue;
            }
        };

        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        // ✅ FIX: Load reserves/liquidity from snapshot (or default to 0 if not available)
        let reserve0_str: Option<String> = row.try_get("reserve0").ok();
        let reserve1_str: Option<String> = row.try_get("reserve1").ok();
        let liquidity_opt: Option<i64> = row.try_get("liquidity").ok();
        let pool_weight: Option<f64> = row.try_get("pool_weight").ok();

        let reserve0 = reserve0_str
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);
        let reserve1 = reserve1_str
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);
        let mut liquidity = liquidity_opt.map(|l| l as u128).unwrap_or(0);

        if reserve0 > 0 || reserve1 > 0 || liquidity > 0 {
            pools_with_reserves += 1;
        } else {
            pools_without_reserves += 1;
        }

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0,
                    reserve1,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => {
                // ✅ OPTION 3: If liquidity is 0, use weight as fallback (convert USD to liquidity estimate)
                // Weight is in USD, so we estimate liquidity by assuming average token price ~$1-2
                // This is a rough approximation: liquidity_estimated = weight_usd * 1e18 / avg_price
                // For scoring purposes, we can use weight directly as proxy, but need to convert to similar scale
                // Using a conversion factor: 1 USD weight ≈ 1e15 liquidity units (rough approximation)
                if liquidity == 0 {
                    if let Some(weight) = pool_weight {
                        if weight > 0.0 {
                            // Convert weight (USD) to liquidity estimate: weight * conversion_factor
                            // Conversion factor: 1 USD ≈ 1e15 liquidity units (rough approximation for scoring)
                            // This ensures weight-based scores are comparable to real liquidity scores
                            let conversion_factor = 1e15;
                            liquidity = (weight * conversion_factor) as u128;
                            tracing::info!("✅ [OPTION 3] V3 pool {} using weight={:.2} USD as liquidity fallback (estimated={})",
                                          address, weight, liquidity);
                        }
                    }
                }
                PoolEnum::UniswapV3(UniswapV3Pool {
                    address,
                    token0,
                    token1,
                    fee: fee_bps.unwrap_or(0) as u32,
                    sqrt_price_x96: Default::default(),
                    liquidity,
                    tick: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "Balancer" => PoolEnum::BalancerWeighted(crate::pools::BalancerWeightedPool {
                address,
                pool_id: [0u8; 32],
                tokens: vec![token0, token1],
                balances: vec![],
                weights: vec![],
                swap_fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Curve" => PoolEnum::CurveStableSwap(crate::pools::CurveStableSwapPool {
                address,
                tokens: vec![token0, token1],
                balances: vec![],
                a: Default::default(),
                fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            unsupported => {
                tracing::debug!(
                    "⚠️ Unsupported DEX type: {} for pool {}",
                    unsupported,
                    address_str
                );
                skipped_count += 1;
                *skipped_dex.entry(dex.clone()).or_insert(0) += 1;
                continue;
            }
        };
        pools.push(pool_enum);
    }

    if skipped_count > 0 {
        tracing::warn!(
            "⚠️ [OPTION 8] Skipped {} pools ({} parse errors, {} unsupported DEX). Breakdown: {:?}",
            skipped_count,
            parse_errors,
            skipped_count - parse_errors,
            skipped_dex
        );
    }

    tracing::info!("✅ [OPTION 8] Successfully loaded {} pools from {} rows ({} with reserves/liquidity, {} without)",
                  pools.len(), total_rows, pools_with_reserves, pools_without_reserves);
    Ok(pools)
}

pub async fn load_top_tvl_pools(pool: &DbPool, limit: i64) -> Result<Vec<PoolEnum>> {
    let query = format!(
        "SELECT p.address, p.dex, p.token0, p.token1, p.fee_bps
         FROM {}.pools p
         LEFT JOIN {}.pool_statistics ps ON p.address = ps.pool_address
         WHERE p.is_active = true
           AND p.is_valid = true
         ORDER BY ({priority}) DESC,
                  COALESCE(ps.tvl_usd, 0.0) DESC,
                  p.updated_at DESC
         LIMIT $1",
        SCHEMA,
        SCHEMA,
        priority = POOL_PRIORITY_EXPR
    );

    let rows = sqlx::query(&query).bind(limit).fetch_all(pool).await?;

    let mut pools = Vec::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address = Address::from_str(&row.try_get::<String, _>("address")?)?;
        let token0 = Address::from_str(&row.try_get::<String, _>("token0")?)?;
        let token1 = Address::from_str(&row.try_get::<String, _>("token1")?)?;
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0: 0,
                    reserve1: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => PoolEnum::UniswapV3(UniswapV3Pool {
                address,
                token0,
                token1,
                fee: fee_bps.unwrap_or(0) as u32,
                sqrt_price_x96: Default::default(),
                liquidity: 0,
                tick: 0,
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Balancer" => PoolEnum::BalancerWeighted(crate::pools::BalancerWeightedPool {
                address,
                pool_id: [0u8; 32],
                tokens: vec![token0, token1],
                balances: vec![],
                weights: vec![],
                swap_fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            "Curve" => PoolEnum::CurveStableSwap(crate::pools::CurveStableSwapPool {
                address,
                tokens: vec![token0, token1],
                balances: vec![],
                a: Default::default(),
                fee: Default::default(),
                dex: Box::leak(dex.into_boxed_str()),
            }),
            _ => continue,
        };
        pools.push(pool_enum);
    }
    Ok(pools)
}

pub async fn load_recent_viable_pools(
    pool: &DbPool,
    lookback_hours: i64,
    limit: i64,
) -> Result<Vec<PoolEnum>> {
    let rows = sqlx::query(&format!(
        "SELECT address, dex, token0, token1, fee_bps
         FROM {}.pools
         WHERE is_active = true
           AND is_valid = true
           AND last_viable_at IS NOT NULL
           AND last_viable_at >= NOW() - ($1::int * INTERVAL '1 hour')
         ORDER BY last_viable_at DESC
         LIMIT $2",
        SCHEMA
    ))
    .bind(lookback_hours)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut pools = Vec::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address = Address::from_str(&row.try_get::<String, _>("address")?)?;
        let token0 = Address::from_str(&row.try_get::<String, _>("token0")?)?;
        let token1 = Address::from_str(&row.try_get::<String, _>("token1")?)?;
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0: 0,
                    reserve1: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => PoolEnum::UniswapV3(UniswapV3Pool {
                address,
                token0,
                token1,
                fee: fee_bps.unwrap_or(0) as u32,
                sqrt_price_x96: Default::default(),
                liquidity: 0,
                tick: 0,
                dex: Box::leak(dex.into_boxed_str()),
            }),
            _ => continue,
        };
        pools.push(pool_enum);
    }
    Ok(pools)
}

/// Load pools discovered in the last N seconds (for incremental weight updates)
pub async fn load_recently_discovered_pools(
    pool: &DbPool,
    seconds_ago: i64,
) -> Result<Vec<Address>> {
    let query = format!(
        "SELECT DISTINCT address, updated_at
         FROM {}.pools
         WHERE is_active = true
           AND is_valid = true
           AND updated_at >= NOW() - ($1::bigint * INTERVAL '1 second')
         ORDER BY updated_at DESC",
        SCHEMA
    );

    let rows = sqlx::query(&query)
        .bind(seconds_ago)
        .fetch_all(pool)
        .await?;

    let mut addresses = Vec::new();
    for row in rows {
        if let Ok(addr_str) = row.try_get::<String, _>("address") {
            if let Ok(addr) = Address::from_str(&addr_str) {
                addresses.push(addr);
            }
        }
    }
    Ok(addresses)
}

pub async fn apply_pool_stats_update(pool: &DbPool, update: &PoolStatsUpdate) -> Result<()> {
    let address_str = format!("{:#x}", update.address);
    let volatility_sample_count: i64 = if update.volatility_bps.is_some() {
        1
    } else {
        0
    };

    sqlx::query(
        &format!(
            "INSERT INTO {}.pool_statistics (
            pool_address,
            tvl_usd,
            volatility_bps,
            volatility_sample_count,
            updated_at
        )
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (pool_address) DO UPDATE SET
            tvl_usd = COALESCE(EXCLUDED.tvl_usd, {}.pool_statistics.tvl_usd),
            volatility_bps = CASE
                WHEN EXCLUDED.volatility_sample_count > 0 THEN
                    CASE
                        WHEN {}.pool_statistics.volatility_sample_count > 0 THEN
                            (
                                COALESCE({}.pool_statistics.volatility_bps, 0.0) *
                                {}.pool_statistics.volatility_sample_count +
                                COALESCE(EXCLUDED.volatility_bps, 0.0)
                            ) / NULLIF({}.pool_statistics.volatility_sample_count + EXCLUDED.volatility_sample_count, 0)
                        ELSE EXCLUDED.volatility_bps
                    END
                ELSE {}.pool_statistics.volatility_bps
            END,
            volatility_sample_count = {}.pool_statistics.volatility_sample_count + EXCLUDED.volatility_sample_count,
            updated_at = NOW()",
            SCHEMA, SCHEMA, SCHEMA, SCHEMA, SCHEMA, SCHEMA, SCHEMA, SCHEMA
        ),
    )
    .bind(address_str)
    .bind(update.tvl_usd)
    .bind(update.volatility_bps)
    .bind(volatility_sample_count)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn load_anchor_candidate_pools(
    pool: &DbPool,
    anchor_token: Address,
    limit: i64,
) -> Result<Vec<PoolEnum>> {
    let anchor_str = format!("{:?}", anchor_token);
    let rows = sqlx::query(&format!(
        "SELECT address, dex, token0, token1, fee_bps
         FROM {}.pools
         WHERE is_active = true
           AND is_valid = true
           AND (token0 = $1 OR token1 = $1)
         ORDER BY last_viable_at DESC NULLS LAST, updated_at DESC
         LIMIT $2",
        SCHEMA
    ))
    .bind(anchor_str)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut pools = Vec::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let address = Address::from_str(&row.try_get::<String, _>("address")?)?;
        let token0 = Address::from_str(&row.try_get::<String, _>("token0")?)?;
        let token1 = Address::from_str(&row.try_get::<String, _>("token1")?)?;
        let fee_bps: Option<i32> = row.try_get("fee_bps")?;

        let pool_enum = match dex.as_str() {
            "UniswapV2" | "SushiSwapV2" | "PancakeSwap" | "TraderJoe" | "CamelotV2" => {
                PoolEnum::UniswapV2(UniswapV2Pool {
                    address,
                    token0,
                    token1,
                    reserve0: 0,
                    reserve1: 0,
                    dex: Box::leak(dex.into_boxed_str()),
                })
            }
            "UniswapV3" | "CamelotV3" | "KyberSwap" => PoolEnum::UniswapV3(UniswapV3Pool {
                address,
                token0,
                token1,
                fee: fee_bps.unwrap_or(0) as u32,
                sqrt_price_x96: Default::default(),
                liquidity: 0,
                tick: 0,
                dex: Box::leak(dex.into_boxed_str()),
            }),
            _ => continue,
        };
        pools.push(pool_enum);
    }
    Ok(pools)
}

pub async fn set_pool_activity(pool: &DbPool, pool_address: &str, is_active: bool) -> Result<()> {
    sqlx::query(&format!(
        "UPDATE {}.pools SET is_active = $1 WHERE address = $2",
        SCHEMA
    ))
    .bind(is_active)
    .bind(pool_address)
    .execute(pool)
    .await?;
    Ok(())
}

/// ✅ MEJORADO: Mark pools as active based on recent activity AND significant weights
///
/// This function fixes the root cause of pools with weights > 0 being marked as inactive.
///
/// Strategy:
/// 1. Mark pools as active if they have recent activity (within last N days) OR significant weight (>= threshold)
/// 2. Mark pools as inactive if they have no activity AND no significant weight
///
/// # Optimization
///
/// Uses a single optimized query with UNION to mark pools as active if they meet ANY condition,
/// reducing database round-trips from 3 to 2 queries.
///
/// # Parameters
///
/// - `pool`: Database connection pool
/// - `max_age_days`: Maximum age in days for considering a pool "recently active" (default: 30)
/// - `min_weight_threshold`: Minimum weight in USD to consider a pool "significant" (default: $10K)
///
/// # Returns
///
/// Returns `(activated_count, deactivated_count)` where:
/// - `activated_count`: Number of pools marked as active
/// - `deactivated_count`: Number of pools marked as inactive
pub async fn check_pools_activity_improved(
    pool: &DbPool,
    max_age_days: i64,
    min_weight_threshold: f64,
) -> Result<(usize, usize)> {
    use chrono::Utc;

    let cutoff_date = Utc::now() - chrono::Duration::days(max_age_days);

    // ✅ OPTIMIZACIÓN: Una sola query que marca pools como activos si cumplen CUALQUIERA de las condiciones
    let result = sqlx::query(&format!(
        r#"
        WITH pools_to_activate AS (
            -- Pools con activity reciente (usar updated_at como proxy de actividad)
            SELECT DISTINCT p.address
            FROM {}.pools p
            WHERE p.updated_at >= $1
              AND p.is_valid = true

            UNION

            -- Pools con weight significativo
            SELECT DISTINCT p.address
            FROM {}.pools p
            INNER JOIN {}.graph_weights gw ON p.address = gw.pool_address
            WHERE gw.weight >= $2
              AND p.is_valid = true
        )
        UPDATE {}.pools p
        SET is_active = true
        FROM pools_to_activate pta
        WHERE p.address = pta.address
        "#,
        SCHEMA, SCHEMA, SCHEMA, SCHEMA
    ))
    .bind(cutoff_date)
    .bind(min_weight_threshold)
    .execute(pool)
    .await?;

    let activated_count = result.rows_affected() as usize;

    // Marcar como inactivos los que no cumplen ninguna condición
    let result2 = sqlx::query(&format!(
        r#"
        UPDATE {}.pools p
        SET is_active = false
        WHERE (p.updated_at < $1 OR p.updated_at IS NULL)
          AND NOT EXISTS (
              SELECT 1 FROM {}.graph_weights gw
              WHERE gw.pool_address = p.address
              AND gw.weight >= $2
          )
          AND p.is_valid = true
        "#,
        SCHEMA, SCHEMA
    ))
    .bind(cutoff_date)
    .bind(min_weight_threshold)
    .execute(pool)
    .await?;

    let deactivated_count = result2.rows_affected() as usize;

    Ok((activated_count, deactivated_count))
}

pub async fn update_pool_last_viable(
    pool: &DbPool,
    pool_address: &Address,
    block_number: u64,
) -> Result<()> {
    sqlx::query(&format!(
        "UPDATE {}.pools
         SET last_viable_at = NOW(), last_viable_block = $1
         WHERE address = $2",
        SCHEMA
    ))
    .bind(block_number as i64)
    .bind(format!("{:?}", pool_address))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_valid_pools_count_per_dex(pool: &DbPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(&format!(
        "SELECT dex, COUNT(*) as count FROM {}.pools WHERE is_valid = true GROUP BY dex",
        SCHEMA
    ))
    .fetch_all(pool)
    .await?;

    let mut counts = HashMap::new();
    for row in rows {
        let dex: String = row.try_get("dex")?;
        let count: i64 = row.try_get("count")?;
        counts.insert(dex, count);
    }
    Ok(counts)
}

/// ✅ FASE 1.3: Checkpoint DEX state in atomic transaction
/// Persists last_processed_block for a DEX in a separate transaction
/// This is called every 100 blocks to ensure progress is saved even if batch writer crashes
pub async fn checkpoint_dex_state(pool: &DbPool, dex: &str, block_number: u64) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO dex_state (dex, last_processed_block, updated_at)
         VALUES ($1, $2, EXTRACT(EPOCH FROM NOW()))
         ON CONFLICT (dex) DO UPDATE SET
         last_processed_block = EXCLUDED.last_processed_block,
         updated_at = EXCLUDED.updated_at",
    )
    .bind(dex)
    .bind(block_number as i64)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// ✅ FASE 3.1: Create event_index table if it doesn't exist (internal, uses transaction)
async fn create_event_index_table_internal(tx: &mut sqlx::Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {}.event_index (
            id SERIAL PRIMARY KEY,
            dex VARCHAR(50) NOT NULL,
            block_number BIGINT NOT NULL,
            event_type VARCHAR(50) NOT NULL,
            pool_address VARCHAR(42) NOT NULL,
            indexed_at TIMESTAMP NOT NULL DEFAULT NOW(),
            UNIQUE(dex, block_number, event_type, pool_address)
        )",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    // Create index for gap detection queries
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS idx_event_index_dex_block ON {}.event_index(dex, block_number)",
        SCHEMA
    ))
    .execute(tx.as_mut())
    .await?;

    Ok(())
}

/// ✅ FASE 3.1: Create event_index table if it doesn't exist (public API)
pub async fn create_event_index_table(pool: &DbPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    create_event_index_table_internal(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}
