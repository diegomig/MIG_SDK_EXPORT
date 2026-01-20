# Funding Matrix: Grant Scope Separation

**Purpose**: Define clear boundaries between grants to avoid double-funding concerns and enable independent auditing.  
**Last Updated**: January 2026  
**Active Grants**: Arbitrum DAO (Questbook) + Uniswap Foundation  
**Application Strategy**: Parallel applications to both programs simultaneously

---

## Grant Scope Separation

### Arbitrum DAO Grant Program - Developer Tooling Domain ($45k USD)

**Program**: Arbitrum DAO Grant Program (via Questbook)  
**Scope**: Core infrastructure and production optimization  
**Crate**: `mig-core` (or current monolith if workspace refactor not yet done)  
**Focus**: Performance, caching, RPC optimization, production readiness

| Component | Funded by Arbitrum | Rationale |
|-----------|-------------------|-----------|
| RPC Pool + Circuit Breaker | ✅ Yes | Core infrastructure for all networks |
| Multicall Batching | ✅ Yes | RPC optimization (network-agnostic) |
| Cache Architecture (Merkle, TTL) | ✅ Yes | Performance optimization core |
| JIT State Fetcher | ✅ Yes | State synchronization core |
| Hot Pool Manager | ✅ Yes | Adaptive caching core |
| Graph Service (base) | ✅ Yes | Weight calculation core |
| PostgreSQL Async Writer | ✅ Yes | Database infrastructure |
| Redis Integration | ✅ Yes | Distributed caching infrastructure |
| Flight Recorder | ✅ Yes | Observability system |
| Stress Testing + CI/CD | ✅ Yes | Production readiness |
| Error Handling (thiserror) | ✅ Yes | SDK industrialization |

**Deliverables**: See `grants/arbitrum/Execution_Plan/MILESTONES.md`

---

### Uniswap Foundation Infrastructure Grant ($75k USD)

**Scope**: Uniswap-specific features, V4 preparation, analytics dashboard  
**Crate**: `mig-adapter-uniswap` (or Uniswap modules if workspace refactor not yet done)  
**Focus**: Mathematical precision, tick math, TWAP, V4 hooks, Uniswap analytics

| Component | Funded by Uniswap | Rationale |
|-----------|------------------|-----------|
| Advanced Tick Math Library | ✅ Yes | Uniswap V3-specific (SqrtPriceX96, tick conversions) |
| TWAP Integration | ✅ Yes | Uniswap V3 oracle-specific |
| Fee Tier Analysis | ✅ Yes | Uniswap V3-specific feature |
| Liquidity Concentration Analysis | ✅ Yes | Uniswap V3-specific (tick ranges) |
| Uniswap V4 Hooks Support | ✅ Yes | Uniswap V4-specific preparation |
| Singleton Pool Architecture | ✅ Yes | Uniswap V4-specific |
| Uniswap Analytics Dashboard | ✅ Yes | Uniswap-focused tooling |
| Protocol Integration Examples | ✅ Yes | Uniswap-specific use cases |

**Deliverables**: See `grants/uniswap/Execution_Plan/ROADMAP_UNISWAP.md`

---

### Unichain Readiness (Track within Uniswap scope)

**Scope**: Unichain network deployment and readiness (if applicable)  
**Funded by**: Uniswap-Arbitrum grant (or separate Unichain track TBD)  
**Focus**: Unichain network configuration, Uniswap V4 deployment on Unichain

| Component | Funded by Uniswap/Unichain | Rationale |
|-----------|---------------------------|-----------|
| Unichain RPC Configuration | ✅ Yes (within Uniswap) | Network deployment |
| Uniswap V4 on Unichain | ✅ Yes (within Uniswap) | Uniswap V4 native home |
| Unichain-specific Optimizations | ✅ Yes (within Uniswap) | Network-specific tuning |

**Note**: Unichain readiness is scoped within the Uniswap grant as "V4 deployment on Uniswap's native chain." If a separate Unichain grants program exists, this scope may be moved to a dedicated grant.

---

## Conditional Components & Parallel Application Strategy

### Workspace Refactor

**Scope**: Refactor monolithic codebase to Rust workspace structure  
**Estimated Effort**: 2-3 weeks  
**Funded by**: First approved grant (Arbitrum OR Uniswap)  
**Budget Allocation**: ~$2-3k (included in Milestone 1 of first grant)

**Parallel Application Scenario:**
- Both grants are being submitted simultaneously (January 2026)
- Workspace refactor will be completed by whichever grant approves first
- If both approve simultaneously, the workspace refactor will be completed in the first milestone of either grant (no duplication)
- The second grant (whichever approves later) will build on top of the workspace structure created by the first

| Grant | Workspace Refactor Responsibility |
|-------|----------------------------------|
| **If Arbitrum approves first** | Arbitrum pays for workspace refactor; Uniswap builds on top of `mig-core` |
| **If Uniswap approves first** | Uniswap pays for workspace refactor; Arbitrum builds on top of `mig-core` |
| **If both approve simultaneously** | First milestone of either grant includes workspace refactor (no duplication) |

**Deliverables**:
- `crates/mig-core/`: Core infrastructure (RPC, cache, graph, database)
- `crates/mig-adapter-uniswap/`: Uniswap-specific modules (tick math, TWAP, V4)
- `crates/mig-adapter-unichain/`: Unichain-specific configuration (if needed)
- `crates/mig-cli/`: CLI tools and benchmarks

---

## Overlap Avoidance Strategy

### Clear Boundaries

1. **Arbitrum**: Core/infrastructure that benefits ALL protocols (network-agnostic)
2. **Uniswap**: Uniswap-specific features that ONLY benefit Uniswap integrations
3. **Unichain**: Network deployment for Uniswap's native chain (within Uniswap scope)

### No Double-Funding

- Each line of code is funded by exactly ONE grant
- Workspace refactor is funded by ONE grant (first approved)
- Shared infrastructure (mig-core) is funded by Arbitrum
- Protocol-specific code (mig-adapter-uniswap) is funded by Uniswap

### Transparency

- This matrix is referenced in all grant applications
- Each grant includes "Other Funding" disclosure section
- Milestones and budgets clearly specify scope boundaries

---

## Audit Trail References

For detailed audit trail per grant:
- **Arbitrum**: `grants/arbitrum/AUDIT_TRAIL.md`
- **Uniswap**: `grants/uniswap/AUDIT_TRAIL.md`

For scope boundaries per grant:
- **Arbitrum**: `grants/arbitrum/SCOPE_BOUNDARY.md`
- **Uniswap**: `grants/uniswap/SCOPE_BOUNDARY.md`

---

## Version History

| Date | Change | Reason |
|------|--------|--------|
| 2026-01-20 | Initial version | Define scope separation for grant applications |
| 2026-01-20 | Added Unichain track | Base paused for 2026, Unichain readiness added to Uniswap scope |

---

**Organization**: MIG Labs  
**Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)  
**License**: MIT OR Apache-2.0
