# Scope Boundary: Arbitrum Foundation Grant

**Grant Program**: Arbitrum Foundation Developer Tooling  
**Amount**: $45,000 USD  
**Focus**: Core infrastructure, performance optimization, production readiness

---

## What This Grant DOES Fund

### Core Infrastructure (mig-core or current monolith)

- RPC Pool (load balancing, circuit breaker, failover)
- Multicall Batching (RPC call optimization)
- Cache Architecture (Merkle tree, TTL differentiation, multi-level cache)
- JIT State Fetcher (state synchronization, fuzzy block matching)
- Hot Pool Manager (adaptive caching, top-K pools)
- Graph Service (base weight calculation, graph updates)
- PostgreSQL Async Writer (batch database updates)
- Redis Integration (distributed caching infrastructure)
- Block Number Cache (RPC call reduction)
- Flight Recorder (observability system)

### Production Readiness

- Error Handling Migration (anyhow → thiserror)
- Memory Optimization (zero-copy hot paths, pre-allocation)
- Stress Testing (24-hour sustained load, memory leak testing)
- CI/CD Pipeline (GitHub Actions, automated testing, coverage)
- Community Infrastructure (contributing guides, templates)
- Sustainability Plan (post-grant maintenance model)

### Documentation

- Complete Rustdocs for core infrastructure
- Architecture documentation updates
- Performance tuning guides
- Stress testing reports
- Benchmark reports (cache hit rate, RPC reduction)

---

## What This Grant does NOT Fund

### Protocol-Specific Features (Funded by Other Grants)

- Uniswap tick math, TWAP, V4 preparation → Uniswap grant
- Uniswap analytics dashboard → Uniswap grant
- Unichain deployment → Uniswap grant (as Unichain readiness track)

### Network-Specific Deployments (Funded by Other Grants)

- Base network port → Base grant (paused for 2026)
- Unichain network deployment → Uniswap grant (as Unichain readiness track)

---

## Modules/Files Touched by This Grant

### Primary Files

- `src/rpc_pool.rs` - RPC pool infrastructure
- `src/multicall.rs` - Multicall batching
- `src/cache/` - Cache architecture (new directory if refactored)
- `src/jit_state_fetcher.rs` - JIT state fetching optimization
- `src/hot_pool_manager.rs` - Hot pool manager
- `src/graph_service.rs` - Graph weight calculation (base)
- `src/postgres_async_writer.rs` - Async database writer
- `src/redis_manager.rs` - Redis integration
- `src/block_number_cache.rs` - Block number caching
- `src/flight_recorder.rs` - Observability system
- `src/error.rs` - Error handling (thiserror migration)

### Secondary Files

- `src/database.rs` - Database optimizations (batch updates)
- `src/settings.rs` - Configuration management
- `.github/workflows/` - CI/CD pipeline
- `CONTRIBUTING.md` - Community infrastructure
- `docs/ARCHITECTURE.md` - Architecture documentation

---

## Conditional: Workspace Refactor

**If this is the first approved grant**:
- Milestone 1 includes workspace refactor (2-3 weeks, ~$2-3k)
- Extract `mig-core` crate from monolith
- Establish crate boundaries and dependencies

**If another grant approves first**:
- Build on existing workspace structure
- No workspace refactor cost (already paid by other grant)

---

## Verification Commands

### Build and Test (Core Infrastructure)

```bash
# Build core infrastructure
cargo build --features redis,observability

# Run tests
cargo test --features redis,observability

# Run benchmark
cargo run --example benchmark_metrics --features redis,observability
```

### Expected Outcomes

- Cache hit rate: ≥80%
- JIT fetch latency: ≤100ms (remote RPC)
- RPC calls per block: ≤30 (from baseline ~158)
- Stress test: 24-hour run without memory leaks

---

## Audit Trail

See `AUDIT_TRAIL.md` for:
- PR links for each milestone
- Git tags for milestone completions
- Benchmark reports
- Code review records

---

**Last Updated**: January 2026  
**Status**: Ready for submission
