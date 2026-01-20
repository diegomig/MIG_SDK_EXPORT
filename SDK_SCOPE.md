# MIG Topology SDK - Scope & Boundaries

This document outlines what is included in the MIG Topology SDK and what is explicitly excluded to maintain clear boundaries between public infrastructure and application-specific logic.

## Included Components

The SDK focuses on **infrastructure for liquidity discovery and validation**:

### Discovery Layer
- Event-driven pool discovery from blockchain events
- Multi-DEX protocol support (Uniswap V2/V3, Balancer, Curve, etc.)
- Block parsing and event extraction
- Streaming discovery architecture

### Normalization Layer
- Unified pool representation across DEX protocols
- DEX adapter pattern (`DexAdapter` trait)
- Factory mapping and protocol detection

### Validation Layer
- Pool quality assessment and filtering
- Bytecode verification
- Liquidity and balance validation
- Blacklist management for corrupted pools

### Graph & State Management
- Weighted liquidity graph
- Just-In-Time (JIT) state synchronization
- Hot pool manager (in-memory cache)
- Block-based caching strategies

### Infrastructure
- RPC provider pool with load balancing
- Multicall batching utilities
- PostgreSQL integration
- Redis caching (optional, feature-gated)
- Flight recorder for observability

## Not Included

The SDK is designed as a **foundation for applications**, not as a complete application itself. The following are explicitly excluded:

### Trading Execution Logic
- Transaction signing and submission
- Gas bidding strategies
- Private transaction routing (Flashbots, Eden)
- Nonce management for trading

### Application-Specific Strategies
- Profit calculation algorithms
- Route optimization for trading
- Sizing strategies
- Risk management for trading

### MEV-Specific Features
- Arbitrage opportunity detection
- Flash loan integration
- Transaction simulation for profit
- Execution timing optimization

## Design Philosophy

The SDK separates **infrastructure concerns** (discovery, validation, state management) from **application concerns** (trading strategies, profit optimization, execution).

This separation allows:
- **Protocols** to build on reliable liquidity data
- **Analytics platforms** to access validated pool information
- **Researchers** to study liquidity topology
- **Infrastructure providers** to offer liquidity services

Without requiring them to implement their own discovery, validation, or state synchronization logic.

## Extensibility

The SDK is designed for extensibility:

1. **New DEX Protocols**: Implement the `DexAdapter` trait
2. **Custom Validators**: Extend `PoolValidator` with domain-specific validation
3. **Graph Algorithms**: Extend `GraphService` with custom path-finding
4. **Cache Strategies**: Implement custom cache backends via `CacheManager`

See `CONTRIBUTING.md` for guidelines on extending the SDK.

