# MIG Topology SDK

**A high-performance Rust library for real-time liquidity mapping and pool validation on Arbitrum One.**

## Why MIG Topology SDK?

**The Problem**: Protocols building on Arbitrum waste 40% of development time dealing with:
- Inconsistent pool data across 10+ DEX protocols
- Low-quality pools that drain gas or corrupt state
- RPC spam from inefficient state fetching

**The Solution**: MIG Topology SDK provides battle-tested infrastructure for real-time liquidity mapping, validated pool data, and ultra-low latency state synchronization—so teams can focus on building their protocol, not maintaining data pipelines.

**Built for**: DeFi protocols, analytics platforms, researchers, and infrastructure providers who need reliable liquidity data without reinventing the wheel.

## Overview

The MIG Topology SDK provides a comprehensive infrastructure layer for discovering, normalizing, and validating liquidity pools across multiple DEX protocols on Arbitrum. It transforms raw blockchain data into a unified, queryable topology graph suitable for DeFi applications, analytics platforms, and protocol integrations.

## Key Features

### 🔍 **Discovery Layer**
- **Event-Driven Discovery**: Scans blockchain events (`PairCreated`, `PoolCreated`) in real-time
- **Multi-DEX Support**: Uniswap V2/V3, Balancer V2/V3, Curve, Camelot, PancakeSwap, TraderJoe, KyberSwap
- **Streaming Architecture**: Block stream with Redis pub/sub for multi-process coordination
- **Deferred Validation Queue**: Priority-based pool validation to optimize RPC usage

### 🔄 **Normalization Layer**
- **Unified Pool Representation**: Standardized `PoolMeta` and `Pool` types across all DEX protocols
- **Adapter Pattern**: Extensible trait-based architecture for adding new DEX protocols
- **Factory Mapping**: Automatic detection of pool creation events from known factory addresses

### ✅ **Validation & Quality**
- **Bytecode Verification**: Validates pool contracts against known bytecode hashes
- **Liquidity Filtering**: Filters out pools without sufficient liquidity or anchor tokens
- **Blacklist Management**: Tracks and excludes corrupted or failing pools
- **Data Quality Metrics**: `StateQuality` classification (Fresh, Stale, Corrupt)

### 📊 **State & Graph Management**
- **Graph Service**: Maintains weighted liquidity graph with real-time updates
- **JIT State Fetching**: Just-In-Time pool state synchronization with fuzzy block matching
- **Hot Pool Manager**: In-memory cache of top-K pools with adaptive refresh rates
- **Block-Based Caching**: Efficient state caching with block number and TTL invalidation
- **SharedPriceCache**: Thread-safe price cache with freshness metadata and background updates
- **Emergency Price Repair**: Automatic recovery of missing prices during weight calculation

### 🛠️ **Infrastructure**
- **RPC Pool**: Load-balanced RPC provider management with automatic failover
- **Multicall Batching**: Optimized batch RPC calls for state fetching
- **PostgreSQL Integration**: Persistent storage for pools, tokens, and graph weights
- **Redis Caching**: Optional Redis backend for distributed caching (feature flag)

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│              MIG Topology SDK                           │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  Discovery Layer                                        │
│  ├── Event Extraction (PairCreated, PoolCreated)        │
│  ├── Block Streaming (WebSocket/HTTP)                  │
│  └── Orchestrator (Discovery Coordination)             │
│                                                         │
│  Normalization Layer                                    │
│  ├── DEX Adapters (Uniswap, Balancer, Curve, ...)      │
│  └── Pool Representation (Unified Types)               │
│                                                         │
│  Validation Layer                                       │
│  ├── Pool Validator (Bytecode, Metadata)               │
│  ├── Data Quality Validator (StateQuality)             │
│  └── Blacklist Manager (Corruption Tracking)           │
│                                                         │
│  Graph & State Layer                                    │
│  ├── Graph Service (Liquidity Weight Calculation)     │
│  ├── JIT State Fetcher (On-Demand State Sync)         │
│  └── Hot Pool Manager (In-Memory Cache)                │
│                                                         │
│  Infrastructure                                         │
│  ├── RPC Pool (Load Balancing)                         │
│  ├── Multicall (Batch RPC Optimization)                │
│  ├── Database (PostgreSQL)                              │
│  └── Redis (Optional Distributed Cache)                │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

## Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
mig-topology-sdk = { version = "0.1", features = ["redis", "observability"] }
```

### Basic Usage

See the [examples](examples/) directory for complete, runnable examples:

- **`basic_setup.rs`**: Initialize the SDK with all components
- **`liquidity_path.rs`**: Find liquidity paths between tokens
- **`realtime_updates.rs`**: Monitor real-time graph updates

Quick example:

```rust
use mig_topology_sdk::{
    orchestrator::Orchestrator,
    graph_service::GraphService,
    settings::Settings,
    // ... other imports
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize SDK (see examples/basic_setup.rs for full setup)
    let settings = Settings::new()?;
    // ... initialize components
    
    // Run discovery
    orchestrator.run_discovery_cycle().await?;
    
    // Query graph
    let weight = graph_service.get_weight(&pool_address);
    
    Ok(())
}
```

**For complete examples, see:**
- [`examples/basic_setup.rs`](examples/basic_setup.rs) - Full SDK initialization
- [`examples/liquidity_path.rs`](examples/liquidity_path.rs) - Finding liquidity paths
- [`examples/realtime_updates.rs`](examples/realtime_updates.rs) - Real-time monitoring

### Migrating Data from arbitrage-bot-v2

If you're migrating from the original `arbitrage-bot-v2` project, you can migrate your existing database data using the provided migration scripts.

**Quick migration:**
```bash
cd scripts
pip install -r requirements.txt
python migrate_db_data.py
```

**What gets migrated:**
- ✅ Tokens, pools, dex_state, pool_state_snapshots
- ✅ Token relations, audit logs, graph weights
- ✅ Pool statistics (compatible columns only)
- ✅ DEX statistics, configurations, event index

**What doesn't get migrated:**
- ❌ Trading-specific tables (opportunities, executions, route_catalog)

See [`docs/DB_MIGRATION_GUIDE.md`](docs/DB_MIGRATION_GUIDE.md) for detailed instructions.

## Supported DEX Protocols

- **Uniswap V2** (Uniswap, SushiSwap, PancakeSwap, TraderJoe)
- **Uniswap V3** (Uniswap, Camelot, KyberSwap)
- **Balancer V2** (Weighted Pools)
- **Balancer V3** (Managed Pools)
- **Curve** (StableSwap, Tricrypto)

## Use Cases

1. **Protocol Integrations**: Real-time liquidity data for lending, derivatives, and yield protocols
2. **Analytics Platforms**: Topology graph for DeFi analytics and visualization
3. **Research & Development**: Foundation for liquidity routing, MEV research, and protocol design
4. **Infrastructure Providers**: Core data layer for DeFi infrastructure services

## Performance

- **Discovery Latency**: <2s per block (event extraction + validation)
- **State Fetch Latency**: <100ms for JIT state synchronization (with local node)
- **Graph Update Latency**: <500ms for weight recalculation (2000 pools)
- **Cache Hit Rate Target**: >80% (fuzzy block matching)

## Documentation

Comprehensive documentation is available in the `docs/` directory:

- **[Architecture](docs/ARCHITECTURE.md)**: Technical architecture, data flow, concurrency model
- **[Deployment](docs/DEPLOYMENT.md)**: Docker setup, PgBouncer configuration, local node setup
- **[DB Migration](docs/DB_MIGRATION_GUIDE.md)**: Migrating data from arbitrage-bot-v2 to MIG_SDK_EXPORT
- **[Troubleshooting](docs/TROUBLESHOOTING.md)**: Common compilation issues and solutions (especially Windows)
- **[WSL Compilation](docs/WSL_COMPILATION.md)**: Compiling and running from WSL (Windows Subsystem for Linux)
- **[Validation](docs/VALIDATION.md)**: Pool validation criteria and quality assurance
- **[Benchmarks](docs/BENCHMARKS.md)**: Performance metrics from controlled testing scenarios
- **[Flight Recorder](docs/FLIGHT_RECORDER.md)**: Event capture system for observability
- **[Roadmap](docs/ROADMAP.md)**: Research challenges and future enhancements

## Roadmap

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for detailed research challenges and future enhancements.

**Phase 0 (Completed)**: R&D Foundation  
**Phase 1 (Completed)**: Core discovery, normalization, and validation  
**Phase 2 (In Progress)**: JIT State Synchronization optimization for ultra-low latency (<200ms)  
**Phase 3 (Planned)**: Unichain readiness (via Uniswap scope) + multi-chain foundations  
**Phase 4 (Planned)**: Graph query API and subscription system

## License

MIT OR Apache-2.0

## Contributing

We welcome contributions! Please see [`CONTRIBUTING.md`](CONTRIBUTING.md) for:

- Coding standards (rustfmt, clippy)
- Testing requirements (unit, integration, property-based)
- Documentation guidelines (rustdoc)
- Pull request process

## Examples

The SDK includes three complete examples:

1. **Basic Setup** (`examples/basic_setup.rs`): Initialize all SDK components
2. **Liquidity Path** (`examples/liquidity_path.rs`): Find paths between tokens
3. **Real-Time Updates** (`examples/realtime_updates.rs`): Monitor graph updates

Run examples with:

```bash
cargo run --example basic_setup
cargo run --example liquidity_path -- WETH_ADDRESS USDC_ADDRESS
cargo run --example realtime_updates
```

---

**Built with ❤️ by MIG Labs**

