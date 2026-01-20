# Deployment Guide

## Docker Infrastructure

The SDK includes Docker Compose configuration for local development and testing. This provides PostgreSQL and Redis services required by the SDK.

### Quick Start with Docker

1. **Navigate to docker_infrastructure directory:**
   ```bash
   cd docker_infrastructure
   ```

2. **Configure environment variables:**
   ```bash
   cp .env.example .env
   # Edit .env if needed (defaults work for development)
   ```

3. **Start services:**
   ```bash
   docker compose up -d
   ```

4. **Verify services are running:**
   ```bash
   docker compose ps
   ```

5. **Set environment variables for SDK:**
   ```bash
   export DATABASE_URL="postgresql://mig_topology_user:mig_topology_pass@localhost:5432/mig_topology"
   export REDIS_URL="redis://localhost:6379"
   ```

6. **Run SDK examples or service:**
   ```bash
   cd ..
   # Run example
   cargo run --example basic_setup --features redis,observability
   
   # Or run background service (continuous operation)
   cargo run --bin background_discoverer
   ```

Migrations are executed automatically when PostgreSQL starts for the first time. For detailed information, see [`docker_infrastructure/README.md`](../docker_infrastructure/README.md).

### Services

- **PostgreSQL**: Port 5432, Database: `mig_topology`, User: `mig_topology_user`
- **Redis**: Port 6379 (optional, requires `redis` feature flag)

### Stopping Services

```bash
cd docker_infrastructure
docker compose stop      # Stop services (data preserved)
docker compose down      # Stop and remove containers (data preserved)
docker compose down -v   # Stop and remove everything (⚠️ deletes data)
```

## PgBouncer Setup

### Configuration

PgBouncer is automatically detected when `DATABASE_URL` contains:
- `pgbouncer` in the URL
- Port `6432` (PgBouncer default)
- `pgbouncer=true` parameter

### Recommended Configuration

```ini
[databases]
mig_topology = host=postgres port=5432 dbname=mig_topology

[pgbouncer]
pool_mode = transaction
max_client_conn = 1000
default_pool_size = 20
```

### Benefits

- **Connection Pooling**: Reduces PostgreSQL connection overhead
- **Transaction Mode**: Recommended for this use case (single transaction per request)
- **Scalability**: Supports up to 1000 client connections with only 20 PostgreSQL connections

## Local Node Configuration

### Setup

1. Deploy full Arbitrum/Ethereum node on bare metal (Virginia, US recommended)
2. Configure RPC URL in `Config.toml`:
   ```toml
   [rpc]
   http_urls = ["http://127.0.0.1:8545"]
   ```

### Benefits

- **Low Latency**: <10ms RPC calls (vs 50-200ms for remote providers)
- **No Rate Limits**: Unlimited RPC calls
- **Real-time Updates**: Direct access to block stream

### Health Checks

Local nodes receive proactive health checks every 5 seconds (vs normal interval for remote providers).

## Write Batching

### Configuration

Write batching is configured in `PostgresAsyncWriter`:
- **Batch Size**: 1000 items OR 100ms interval (whichever comes first)
- **Checkpointing**: Every 100 blocks (atomic transaction)

### Benefits

- **Reduced Database Load**: Batches multiple writes into single transactions
- **Fault Tolerance**: Checkpointing ensures progress is saved even on crash
- **Performance**: Non-blocking writes for fast-path operations

## WebSocket Block Subscription

### Setup

WebSocket block subscription is automatically enabled when:
- `features.enable_websocket_blocks = true` (default)
- Local node or WebSocket-capable RPC provider available

### Fallback

If WebSocket disconnects for >5 seconds, polling fallback activates (1s interval).

### Benefits

- **Real-time Updates**: Block notifications <50ms latency
- **Reduced RPC Calls**: No polling needed when WebSocket active
- **Automatic Recovery**: Exponential backoff reconnection

## Background Discoverer Service

### Overview

The SDK includes a production-ready background service (`background_discoverer`) that continuously runs discovery cycles and graph weight updates.

### Running the Service

**Prerequisites:**
- PostgreSQL and Redis running (via Docker Compose or external services)
- Environment variables configured (`DATABASE_URL`, `SDK_RPC_HTTP_URLS`, etc.)
- Configuration file (`Config.toml`) in project root

**Start the service:**

```bash
# From project root
cargo run --bin background_discoverer
```

The service will:
- Initialize all SDK components (RPC pool, database, orchestrator, graph service)
- Start Flight Recorder for event capture (if enabled)
- Run discovery cycles periodically (default: every 120 seconds)
- Update graph weights periodically (default: every 60 seconds)
- Handle graceful shutdown on Ctrl+C

**Configuration:**

Intervals are configured in `Config.toml`:

```toml
[discovery]
interval_seconds = 120  # Discovery cycle interval

[graph]
update_interval_seconds = 60  # Graph weight update interval
```

**Graceful Shutdown:**

Press `Ctrl+C` to stop the service gracefully. The service will:
- Cancel all background tasks
- Flush any pending Flight Recorder events
- Close database connections cleanly

**Flight Recorder Output:**

The service automatically captures events to:
- `logs/flight_recorder_YYYYMMDD_HHMMSS.jsonl`

This enables observability and benchmark metrics collection. See [`docs/BENCHMARKS.md`](BENCHMARKS.md) for details on generating benchmark reports.

**Production Deployment:**

For production deployments, consider:
- Running as a systemd service (Linux) or Windows Service
- Using process managers (supervisord, systemd)
- Setting up log rotation for Flight Recorder output
- Monitoring service health and restarting on failure

See [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) for detailed architecture information.

