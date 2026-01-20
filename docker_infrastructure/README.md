# Docker Infrastructure - MIG Topology SDK

This directory contains Docker infrastructure for running PostgreSQL and Redis services required by the MIG Topology SDK.

## 📋 Contents

- `docker-compose.yml` - Service configuration (PostgreSQL, Redis)
- `.env.example` - Environment variables template
- `scripts/` - Initialization scripts (Linux/Mac and Windows)
- `README.md` - This documentation

## 🚀 Quick Start

### 1. Configure Environment Variables

```bash
# Copy the example file
cp .env.example .env

# Edit .env with your credentials (optional, defaults work for development)
```

### 2. Start Services

```bash
# Start services in background
docker compose up -d

# View logs
docker compose logs -f

# Check service status
docker compose ps
```

### 3. Execute Migrations

#### Option A: Automatic (Recommended)
Migrations are executed automatically when PostgreSQL starts if they are in `../migrations/`.

#### Option B: Manual (Linux/Mac)
```bash
chmod +x scripts/init_db.sh
./scripts/init_db.sh
```

#### Option C: Manual (Windows PowerShell)
```powershell
.\scripts\init_db.ps1
```

### 4. Verify Everything Works

```bash
# Verify PostgreSQL
docker compose exec postgres psql -U mig_topology_user -d mig_topology -c "SELECT current_database(), current_schema();"

# Verify Redis
docker compose exec redis redis-cli ping
```

### 5. Stop Services

```bash
# Stop services (keeps volumes)
docker compose stop

# Stop and remove containers (keeps volumes)
docker compose down

# Stop and remove containers and volumes (⚠️ deletes data)
docker compose down -v
```

## 📊 Services

### PostgreSQL
- **Port:** 5432 (configurable via `POSTGRES_PORT`)
- **Database:** `mig_topology`
- **User:** `mig_topology_user`
- **Password:** Configurable via `POSTGRES_PASSWORD` (default: `mig_topology_pass`)
- **Volume:** `postgres_data` (persistent)

### Redis
- **Port:** 6379 (configurable via `REDIS_PORT`)
- **Volume:** `redis_data` (persistent)
- **Max Memory:** 512MB
- **Policy:** `allkeys-lru`

## 🔄 Migrations

SQL migrations are located in `../migrations/` (project root) and are executed in alphabetical order.

### Main Migration: `001_rename_schema_arbitrage_to_mig_topology.sql`

This migration:
1. Creates the `mig_topology` schema if it doesn't exist
2. Enables required extensions (uuid-ossp)

For new installations, the migration simply creates the `mig_topology` schema with all required tables and extensions.

## 🔧 Configuration

### Main Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `POSTGRES_PASSWORD` | PostgreSQL password | `mig_topology_pass` |
| `POSTGRES_PORT` | PostgreSQL port | `5432` |
| `REDIS_PORT` | Redis port | `6379` |
| `DATABASE_URL` | Complete connection URL | See `.env.example` |
| `REDIS_URL` | Redis connection URL | `redis://localhost:6379` |

### Customize Configuration

1. Copy `.env.example` to `.env`
2. Edit `.env` with your values
3. Restart services: `docker compose down && docker compose up -d`

## 🛠️ Troubleshooting

### PostgreSQL Not Starting

```bash
# View detailed logs
docker compose logs postgres

# Check if port is in use
netstat -an | grep 5432  # Linux/Mac
Get-NetTCPConnection -LocalPort 5432  # Windows
```

### Redis Not Responding

```bash
# View logs
docker compose logs redis

# Test connection manually
docker compose exec redis redis-cli ping
```

### Migrations Fail

```bash
# Verify PostgreSQL is ready
docker compose exec postgres pg_isready -U mig_topology_user

# Execute migration manually
docker compose exec postgres psql -U mig_topology_user -d mig_topology -f /docker-entrypoint-initdb.d/001_rename_schema_arbitrage_to_mig_topology.sql
```

### Clean Everything and Start Fresh

```bash
# ⚠️ WARNING: This deletes all data
docker compose down -v
docker compose up -d
```

## 📝 Notes

- **Development:** Defaults are safe for local development
- **Production:** Change all passwords and use Docker secrets
- **Volumes:** Data persists between container restarts
- **Network:** Services are on the `mig_topology_network` network

## 🔗 References

- [Docker Compose Documentation](https://docs.docker.com/compose/)
- [PostgreSQL Docker Image](https://hub.docker.com/_/postgres)
- [Redis Docker Image](https://hub.docker.com/_/redis)
