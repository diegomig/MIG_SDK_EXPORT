# Technical Documentation & Codebase Links

**Grant Program**: Uniswap Foundation Infrastructure Grant  
**Project**: MIG Topology SDK - Uniswap Ecosystem Optimization  
**Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)

---

## Repository Information

- **GitHub Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)
- **License**: MIT OR Apache-2.0 (Open Source)
- **Language**: Rust (backend), JavaScript (dashboard frontend)
- **Platform**: Arbitrum One
- **Focus**: Uniswap V2/V3/V4

---

## Uniswap-Specific Documentation

### Current Implementation

1. **Uniswap V2 Adapter**: `src/adapters/uniswap_v2.rs`
   - PairCreated event discovery
   - Pool state fetching
   - Factory address support

2. **Uniswap V3 Adapter**: `src/adapters/uniswap_v3.rs`
   - PoolCreated event discovery
   - Pool state fetching (sqrtPriceX96, liquidity, tick)
   - Fee tier support

3. **Uniswap Contracts**: `src/contracts/`
   - `i_uniswap_v2_factory.rs`: V2 factory interface
   - `i_uniswap_v2_pair.rs`: V2 pair interface
   - `i_uniswap_v3_factory.rs`: V3 factory interface
   - `i_uniswap_v3_pool.rs`: V3 pool interface
   - `uniswap_v3.rs`: V3 pool and quoter utilities

4. **V3 Math Utilities**: `src/v3_math.rs`
   - Basic V3 math functions
   - Tick calculations (foundation for advanced tick math)

### Core Documentation

1. **[ARCHITECTURE.md](../../docs/ARCHITECTURE.md)**
   - Technical architecture overview
   - DEX adapter pattern (Uniswap adapters)
   - Component interactions

2. **[ROADMAP.md](../../docs/ROADMAP.md)**
   - Development phases
   - Current state (Phase 1 completed)
   - Future enhancements (Phase 2+)

3. **[SDK_SCOPE.md](../../SDK_SCOPE.md)**
   - Scope and boundaries
   - Uniswap V2/V3 support (included)
   - Trading execution (excluded)

---

## Grant Deliverables Documentation

### Milestone 1: Enhanced Uniswap V2/V3 Support

**New Documentation** (to be created):
- `docs/UNISWAP_FEATURES.md`: Uniswap-specific features guide
- `src/uniswap/tick_math.rs`: Advanced tick math library (rustdocs)
- `src/uniswap/twap.rs`: TWAP integration (rustdocs)
- `src/uniswap/fee_analysis.rs`: Fee tier analysis (rustdocs)

**Enhanced Documentation**:
- `docs/ARCHITECTURE.md`: Uniswap-specific architecture enhancements
- Rustdocs: Complete API documentation for Uniswap modules

### Milestone 2: Uniswap Analytics Dashboard

**New Documentation** (to be created):
- `examples/uniswap_dashboard/README.md`: Dashboard setup guide
- `examples/uniswap_dashboard/API.md`: API documentation
- `docs/DASHBOARD_DEPLOYMENT.md`: Deployment guide

**Dashboard Components**:
- Backend: `examples/uniswap_dashboard/backend/`
- Frontend: `examples/uniswap_dashboard/frontend/`

### Milestone 3: Uniswap V4 Preparation

**New Documentation** (to be created):
- `docs/UNISWAP_V4.md`: Uniswap V4 preparation guide
- `docs/PROTOCOL_INTEGRATION.md`: Protocol integration tutorials
- `src/uniswap/v4_hooks.rs`: V4 hooks support (rustdocs)
- `src/uniswap/singleton.rs`: Singleton pool support (rustdocs)

---

## Uniswap Protocol References

### Official Documentation

- **Uniswap V2**: [Uniswap V2 Documentation](https://docs.uniswap.org/protocol/V2/introduction)
- **Uniswap V3**: [Uniswap V3 Documentation](https://docs.uniswap.org/protocol/introduction)
- **Uniswap V4**: [Uniswap V4 Documentation](https://docs.uniswap.org/protocol/V4/introduction) (when available)

### Reference Implementations

- **Uniswap V3 Core**: [GitHub Repository](https://github.com/Uniswap/v3-core)
- **Uniswap V3 Periphery**: [GitHub Repository](https://github.com/Uniswap/v3-periphery)
- **Tick Math Reference**: Uniswap V3 Core implementation (for validation)

---

## Testing & Validation

### Tick Math Validation

**Requirement**: 100% match with Uniswap V3 reference implementation

**Validation Process**:
- Property-based tests against Uniswap V3 reference
- Mathematical correctness validation
- Edge case testing (tick boundaries, overflow scenarios)

### TWAP Integration Testing

**Test Scenarios**:
- TWAP oracle queries
- Historical price data access
- Error handling and fallbacks

### V4 Preparation Testing

**Test Scenarios** (when V4 testnet available):
- Hooks discovery and validation
- Singleton pool discovery
- Hook-based pool patterns

---

## API Documentation

### Rustdocs

**Current Status**: Basic rustdocs for Uniswap adapters

**Phase 2 Target**: 100% rustdocs coverage for Uniswap modules

**Build Instructions**:
```bash
cargo doc --no-deps --open
```

### Dashboard API

**API Type**: GraphQL or REST (to be determined)

**Documentation**: `examples/uniswap_dashboard/API.md` (to be created)

---

## Running Tests

```bash
# All tests
cargo test

# Uniswap-specific tests
cargo test uniswap

# Tick math tests
cargo test tick_math

# TWAP integration tests
cargo test twap
```

---

## Development Setup

### Prerequisites

- Rust 1.75+ (stable)
- Node.js 18+ (for dashboard frontend)
- PostgreSQL 14+ (for data storage)
- Local Arbitrum node (optional, for development)

### Getting Started

```bash
# Clone repository
git clone https://github.com/mig-labs/mig-topology-sdk.git
cd mig-topology-sdk

# Set up environment
cp .env.example .env
# Edit .env with your configuration

# Build
cargo build --release

# Run tests
cargo test

# Run Uniswap examples (after implementation)
cargo run --example uniswap_twap
```

---

## Contributing

**Contributing Guide**: [CONTRIBUTING.md](../../CONTRIBUTING.md)

**Uniswap-Specific Guidelines**:
- Tick math must match Uniswap V3 reference implementation
- TWAP integration must follow oracle best practices
- V4 preparation must align with Uniswap V4 architecture

---

## License

**License**: MIT OR Apache-2.0 (Open Source)

Users can choose either license for maximum compatibility.

---

## Contact & Support

**Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)  
**Issues**: [GitHub Issues](https://github.com/mig-labs/mig-topology-sdk/issues)  
**Focus**: Uniswap V2/V3/V4 on Arbitrum

---

## Planned Workspace Structure (Grant-Funded Refactor)

**Current Status:** The codebase is currently monolithic but fully functional. All grants will benefit from the workspace refactor, ensuring clean module boundaries and shared core infrastructure.

Upon grant approval, the codebase will be restructured as a Rust workspace:

```
mig-topology-sdk/
├── Cargo.toml (workspace)
├── crates/
│   ├── mig-core/          (Funded by Arbitrum Foundation Grant)
│   ├── mig-adapter-uniswap/  (Funded by Uniswap-ARB Grant)
│   │   └── Uniswap-specific: tick math, V4 hooks, analytics, TWAP
│   ├── mig-adapter-base/     (Funded by Base Builders Grant)
│   └── mig-adapter-arbitrum/ (Multi-DEX adapters)
└── docs/grants/
```

**Benefits:**
- **Modular**: Each grant funds a specific crate with clear boundaries
- **Maintainable**: Bug fixes in core benefit all adapters automatically
- **Professional**: Industry-standard pattern (used by Foundry, Reth, Solana)
- **Publishable**: Crates can be published to crates.io independently
- **Transparent**: Clear separation of funded work per grant

**Timeline:** Workspace refactor will be completed in Milestone 1 of whichever grant approves first (estimated 2-3 weeks). The refactor establishes the foundation for all future development, including Uniswap-specific enhancements.

---

**Last Updated**: January 2025  
**Grant Application**: Uniswap-Arbitrum Grant Program (UAGP)  
**Phase**: Uniswap Ecosystem Optimization
