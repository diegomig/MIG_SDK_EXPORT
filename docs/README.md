# MIG Topology SDK - Documentation Index

**Last Updated**: January 2026  
**Purpose**: Navigation guide for technical documentation

---

## Quick Navigation

### For Grant Reviewers

Start here to understand the project:
- **[Main README](../README.md)** - Project overview and quick start
- **[Grant Applications](../grants/README.md)** - Arbitrum + Uniswap grant proposals
- **[Benchmark Executive Summary](../BENCHMARK_EXECUTIVE_SUMMARY.md)** - Performance metrics for grants

### For Developers

Core technical documentation:
- **[Architecture](ARCHITECTURE.md)** - Technical architecture, data flow, concurrency
- **[Deployment](DEPLOYMENT.md)** - Docker setup, infrastructure configuration
- **[Contributing](../CONTRIBUTING.md)** - Coding standards, testing, PR process

### For Performance Analysis

Benchmarks and metrics:
- **[Benchmarks](BENCHMARKS.md)** - Performance metrics and analysis
- **[Benchmark Quick Start](../BENCHMARK_QUICK_START.md)** - How to run benchmarks
- **[Benchmark Notes](benchmarks/notes/)** - Detailed analysis and troubleshooting

---

## Documentation Categories

### Core Architecture

- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Complete technical architecture
  - 3-layer architecture (API, Core, Infrastructure)
  - Data flow and concurrency model
  - Cache architecture and state management
  - Price feed system with SharedPriceCache
  - Multi-DEX adapter pattern

### Infrastructure & Deployment

- **[DEPLOYMENT.md](DEPLOYMENT.md)** - Infrastructure setup
  - Docker Compose configuration
  - PostgreSQL and Redis setup
  - PgBouncer configuration
  - Local node integration
  - Background discoverer service

- **[WSL_COMPILATION.md](WSL_COMPILATION.md)** - Windows/WSL compilation guide
- **[WSL_QUICK_START.md](WSL_QUICK_START.md)** - WSL quick start

### Performance & Optimization

- **[BENCHMARKS.md](BENCHMARKS.md)** - Performance benchmarks
  - Latency metrics (discovery, state fetch, graph update)
  - Memory metrics
  - Cache analysis
  - RPC optimization results

- **[Flight Recorder](FLIGHT_RECORDER.md)** - Observability system
  - Event capture architecture
  - Event types and metadata
  - Analysis tools

### Validation & Quality

- **[VALIDATION.md](VALIDATION.md)** - Pool validation criteria
  - Bytecode verification
  - Liquidity filtering
  - Quality metrics
  - Blacklist management

### Development & Planning

- **[ROADMAP.md](ROADMAP.md)** - Development roadmap
  - Phase 0: R&D Foundation (completed)
  - Phase 1: Core Implementation (completed)
  - Phase 2: Ultra-Low Latency Optimization (in progress, grant-funded)
  - Future phases

- **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI-first development methodology
- **[DECISIONS.md](DECISIONS.md)** - Architecture decisions and rationale

### Database & Migration

- **[DB_MIGRATION_GUIDE.md](DB_MIGRATION_GUIDE.md)** - Database migration from arbitrage-bot-v2
- **[FEATURE_FLAGS.md](FEATURE_FLAGS.md)** - Feature flags and configuration

### Troubleshooting

- **[TROUBLESHOOTING.md](TROUBLESHOOTING.md)** - Common issues and solutions

---

## Analysis & Reports (Historical)

These documents are analysis artifacts from development iterations:

- **[ANALYSIS_REVIEW.md](ANALYSIS_REVIEW.md)** - Code analysis reviews
- **[COMPLETE_SYSTEM_ANALYSIS.md](COMPLETE_SYSTEM_ANALYSIS.md)** - System-wide analysis
- **[WEIGHT_ANALYSIS_REPORT.md](WEIGHT_ANALYSIS_REPORT.md)** - Weight calculation analysis
- **[THROUGHPUT_ANALYSIS.md](THROUGHPUT_ANALYSIS.md)** - Throughput optimization analysis
- **[ROOT_CAUSE_ANALYSIS.md](ROOT_CAUSE_ANALYSIS.md)** - Issue root cause analyses

---

## Benchmark Notes (Detailed Analysis)

Detailed benchmark analysis and troubleshooting:

- `benchmarks/notes/BENCHMARK_ANALYSIS.md` - Price feed problem analysis
- `benchmarks/notes/BENCHMARK_FIXES.md` - Benchmark issue fixes
- `benchmarks/notes/BENCHMARK_PRICE_FEED_ISSUE.md` - Price feed debugging
- `benchmarks/notes/BENCHMARK_NEXT_STEPS.md` - Optimization next steps
- `benchmarks/notes/BENCHMARK_METRICS_SUMMARY.md` - Metrics summaries
- `benchmarks/notes/BENCHMARK_RESULTS_SUMMARY.md` - Results summaries
- `benchmarks/notes/BENCHMARK_STATUS.md` - Benchmark status tracking
- `benchmarks/notes/BENCHMARK_CHECKLIST.md` - Benchmark validation checklist

---

## Document Status

### Core Docs (Maintained)

These documents are actively maintained and up-to-date:
- ✅ ARCHITECTURE.md
- ✅ DEPLOYMENT.md
- ✅ BENCHMARKS.md
- ✅ VALIDATION.md
- ✅ FLIGHT_RECORDER.md
- ✅ ROADMAP.md

### Historical/Analysis Docs (Reference)

These documents are kept for reference but may not reflect the current state:
- Reference: Analysis reports, troubleshooting notes, migration guides

---

## Quick Links

- **Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)
- **Grants**: [grants/README.md](../grants/README.md)
- **Examples**: [examples/](../examples/)
- **Tests**: [tests/](../tests/)

---

**Organization**: MIG Labs  
**License**: MIT OR Apache-2.0
