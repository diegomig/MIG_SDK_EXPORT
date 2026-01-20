# Scope Boundary: Uniswap-Arbitrum Grant

**Grant Program**: Uniswap Foundation Infrastructure Grant  
**Amount**: $75,000 USD  
**Focus**: Uniswap V2/V3/V4 support, analytics dashboard, protocol integrations

---

## What This Grant DOES Fund

### Uniswap-Specific Features (mig-adapter-uniswap or Uniswap modules)

- Advanced Tick Math Library (tick-to-price, SqrtPriceX96 calculations)
- TWAP Integration (Uniswap V3 oracle integration)
- Fee Tier Analysis (fee tier optimization, multi-fee tier discovery)
- Liquidity Concentration Analysis (tick range analysis)
- Uniswap V4 Hooks Architecture Support (hooks discovery, validation)
- Singleton Pool Architecture Support (V4 singleton pool discovery)
- V4 Integration Patterns (adapter patterns for V4 launch)

### Uniswap Analytics Dashboard

- Rust Backend API (real-time data streaming, GraphQL/REST)
- JavaScript Frontend (React/Vue.js, topology visualization, analytics)
- Pool Analytics (liquidity, volume, fees)
- Historical Data Visualization (charts, trends)

### Protocol Integration Examples

- Lending Protocol Example (collateral valuation using Uniswap pools)
- Analytics Platform Integration Example
- Research Tool Example (MEV research, liquidity analysis)

### Unichain Readiness (Track within Uniswap)

- Unichain RPC configuration
- Uniswap V4 deployment on Unichain
- Unichain-specific optimizations (if needed)

**Note**: Unichain readiness is scoped as "V4 deployment on Uniswap's native chain" within this grant. If a separate Unichain grants program becomes available, this scope may be moved to a dedicated grant.

---

## What This Grant does NOT Fund

### Core Infrastructure (Funded by Arbitrum Grant)

- RPC Pool, Multicall Batching → Arbitrum grant
- Cache Architecture, JIT State Fetcher → Arbitrum grant
- Hot Pool Manager, Graph Service (base) → Arbitrum grant
- PostgreSQL/Redis infrastructure → Arbitrum grant
- Flight Recorder → Arbitrum grant
- Stress testing, CI/CD → Arbitrum grant

---

## Modules/Files Touched by This Grant

### Primary Files (Uniswap-Specific)

- `src/uniswap/tick_math.rs` - Tick math library (NEW)
- `src/uniswap/sqrt_price.rs` - SqrtPriceX96 calculations (NEW)
- `src/uniswap/liquidity.rs` - Liquidity calculations (NEW)
- `src/uniswap/twap.rs` - TWAP oracle integration (NEW)
- `src/uniswap/fee_analysis.rs` - Fee tier analysis (NEW)
- `src/uniswap/v4_hooks.rs` - V4 hooks support (NEW)
- `src/uniswap/singleton.rs` - V4 singleton pool support (NEW)
- `examples/uniswap_dashboard/` - Analytics dashboard (NEW)
- `examples/lending_uniswap.rs` - Lending protocol example (NEW)
- `examples/analytics_integration.rs` - Analytics platform example (NEW)
- `examples/research_tool.rs` - Research tool example (NEW)

### Secondary Files (Enhanced for Uniswap)

- `src/adapters/uniswap_v3.rs` - Enhanced with tick math integration
- `src/price_feeds.rs` - Enhanced with TWAP integration
- `docs/UNISWAP_FEATURES.md` - Uniswap features documentation (NEW)
- `docs/UNISWAP_V4.md` - V4 preparation guide (NEW)

---

## Conditional: Workspace Refactor

**If this is the first approved grant**:
- Milestone 1 includes workspace refactor (2-3 weeks, ~$2-3k ARB)
- Extract `mig-core` and create `mig-adapter-uniswap` crate
- Establish crate boundaries

**If Arbitrum approves first**:
- Build on existing `mig-core` workspace structure
- No workspace refactor cost (already paid by Arbitrum grant)

---

## Verification Commands

### Build and Test (Uniswap-Specific)

```bash
# Build Uniswap modules
cargo build --features redis,observability

# Run Uniswap-specific tests
cargo test --features redis,observability uniswap

# Run dashboard
cargo run --example uniswap_dashboard

# Run protocol integration examples
cargo run --example lending_uniswap
cargo run --example analytics_integration
cargo run --example research_tool
```

### Expected Outcomes

- Tick math accuracy: 100% match with Uniswap V3 reference
- TWAP integration: <100ms latency
- Dashboard: Real-time updates <1s latency
- V4 preparation: Ready for V4 testnet

---

## Audit Trail

See `AUDIT_TRAIL.md` for:
- PR links for each milestone
- Git tags for milestone completions
- Mathematical validation reports
- Dashboard deployment records

---

**Last Updated**: January 2026  
**Status**: Ready for submission
