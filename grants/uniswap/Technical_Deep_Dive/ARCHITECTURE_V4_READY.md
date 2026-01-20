# Architecture: Uniswap V4 Readiness

**Grant Program**: Uniswap Foundation Infrastructure Grant  
**Project**: MIG Topology SDK - Uniswap Ecosystem Optimization

---

## Executive Summary

This document describes how the MIG Topology SDK architecture prepares for **Uniswap V4 launch**, including hooks architecture support, singleton pool architecture, and integration patterns.

---

## Uniswap V4 Overview

### Key V4 Features

1. **Hooks**: Customizable pool logic via hooks (before/after swap, mint, burn)
2. **Singleton Architecture**: Single pool contract for all pools (gas optimization)
3. **Flash Accounting**: Improved gas efficiency for multi-hop swaps
4. **Native ETH Support**: Native ETH support (no WETH wrapping)

### Architecture Impact

**For SDK**:
- **Hooks Discovery**: Discover and validate pools with hooks
- **Singleton Pool Discovery**: Discover pools from singleton contract
- **Hook Integration**: Support for hook-based pool patterns
- **State Fetching**: Adapt state fetching for singleton architecture

---

## Current Architecture (V2/V3)

### Adapter Pattern

**Current Implementation**:
- `UniswapV2Adapter`: Factory-based pool discovery (`PairCreated` events)
- `UniswapV3Adapter`: Factory-based pool discovery (`PoolCreated` events)
- Pool state fetching: Protocol-specific (V2: `getReserves()`, V3: `slot0()`)

### Factory-Based Discovery

```rust
// Current V2/V3 pattern
async fn discover_pools(...) -> Result<Vec<PoolMeta>> {
    let factory = IUniswapV3Factory::new(factory_address, provider);
    let event = factory.event::<PoolCreatedFilter>();
    let logs = event.from_block(start).to_block(end).query().await?;
    // Parse logs to PoolMeta
}
```

---

## V4 Architecture Adaptation

### Singleton Pool Discovery

**V4 Change**: Single pool contract for all pools (no factory events)

**Approach**: Pool discovery via registry or hook discovery

**Implementation Strategy**:
```rust
// V4 singleton pool discovery (preparation)
async fn discover_v4_pools(...) -> Result<Vec<PoolMeta>> {
    // Option 1: Registry-based discovery (if V4 provides registry)
    // Option 2: Hook-based discovery (discover pools via hooks)
    // Option 3: Event-based discovery (if V4 provides pool creation events)
}
```

### Hooks Architecture Support

**V4 Feature**: Pools can have hooks (custom logic)

**SDK Support**:
- **Hook Discovery**: Identify pools with hooks
- **Hook Validation**: Validate hook contracts
- **Hook-Aware State Fetching**: Adapt state fetching for hook-based pools

**Implementation Strategy**:
```rust
pub struct V4PoolMeta {
    pool_key: PoolKey,  // V4 pool identifier
    hook_address: Option<Address>,  // Hook contract (if present)
    hook_flags: u8,  // Hook configuration flags
    // ... other metadata
}
```

### Singleton Pool State Fetching

**V4 Change**: All pools share single contract (singleton)

**Approach**: Pool state fetching via pool key (not address)

**Implementation Strategy**:
```rust
// V4 state fetching (preparation)
async fn fetch_v4_pool_state(
    pool_key: &PoolKey,
    pool_address: Address,  // Singleton contract address
) -> Result<V4PoolState> {
    // Fetch state using pool key
    // Adapt to singleton contract interface
}
```

---

## Architecture Preparation (This Grant)

### Phase 1: Hooks Architecture Foundation

**Objective**: Establish hooks architecture support patterns

**Implementation**:
1. **Hook Discovery Pattern**: Define pattern for discovering pools with hooks
2. **Hook Validation Pattern**: Define pattern for validating hook contracts
3. **Hook-Aware Metadata**: Extend `PoolMeta` to support hook information

**Deliverables**:
- `src/uniswap/v4_hooks.rs`: Hooks architecture support (preparation)
- Hook discovery patterns
- Hook validation patterns

### Phase 2: Singleton Architecture Support

**Objective**: Establish singleton pool architecture support

**Implementation**:
1. **Singleton Pool Discovery**: Define pattern for singleton pool discovery
2. **Pool Key Handling**: Define pattern for pool key management
3. **Singleton State Fetching**: Define pattern for singleton state fetching

**Deliverables**:
- `src/uniswap/singleton.rs`: Singleton pool architecture support
- Singleton discovery patterns
- Singleton state fetching patterns

### Phase 3: V4 Integration Patterns

**Objective**: Define integration patterns for V4 launch

**Implementation**:
1. **V4 Adapter Pattern**: Define V4 adapter implementation pattern
2. **V4 Pool Type**: Define V4 pool type (if needed)
3. **V4 Integration Guide**: Document V4 integration patterns

**Deliverables**:
- V4 adapter implementation pattern
- V4 integration documentation
- V4 testnet validation (when available)

---

## Migration Strategy: V3 → V4

### Compatibility Approach

**Strategy**: Support both V3 and V4 simultaneously

**Implementation**:
- V3 adapter: Continue existing V3 support
- V4 adapter: New V4 adapter alongside V3
- Unified interface: Both adapters implement `DexAdapter` trait

### Pool Type Handling

**Current**:
```rust
pub enum Pool {
    UniswapV2(UniswapV2Pool),
    UniswapV3(UniswapV3Pool),
    // ...
}
```

**V4 Addition** (preparation):
```rust
pub enum Pool {
    UniswapV2(UniswapV2Pool),
    UniswapV3(UniswapV3Pool),
    UniswapV4(UniswapV4Pool),  // V4 pool type
    // ...
}
```

---

## V4 Testnet Validation

### Testnet Preparation

**When V4 Testnet Available**:
1. Deploy V4 adapter implementation
2. Test hooks discovery and validation
3. Test singleton pool discovery
4. Test state fetching with singleton architecture
5. Validate integration patterns

**Validation Criteria**:
- ✅ Hooks discovery: All pools with hooks discovered
- ✅ Singleton discovery: All pools discovered from singleton
- ✅ State fetching: State fetching works with singleton architecture
- ✅ Integration: V4 adapter integrates with SDK architecture

---

## Architecture Benefits

### Extensibility

**Hooks Support**: SDK architecture supports hook-based pools from day one

**Singleton Support**: SDK architecture supports singleton pool architecture

**Future-Proof**: Architecture prepared for V4 launch, enabling rapid V4 integration

### Code Reuse

**Adapter Pattern**: V4 adapter reuses existing adapter pattern

**State Management**: V4 state management reuses existing state management infrastructure

**Validation Logic**: V4 validation reuses existing validation patterns (with V4-specific additions)

---

## Implementation Timeline

### This Grant (4-6 months)

- ✅ Hooks architecture support (preparation)
- ✅ Singleton architecture support
- ✅ V4 integration patterns
- ✅ Documentation and guides

### Post-Grant (V4 Launch)

- 🔄 V4 testnet validation (when available)
- 🔄 V4 mainnet integration (upon launch)
- 🔄 Production support and optimizations

---

## Conclusion

The MIG Topology SDK architecture is **prepared for Uniswap V4 launch** with:

- **Hooks Architecture Support**: Patterns for hook-based pool discovery and validation
- **Singleton Architecture Support**: Patterns for singleton pool discovery and state fetching
- **Extensibility**: Architecture supports V4 integration without major refactoring
- **Future-Proof**: Early preparation enables rapid V4 integration upon launch

With this preparation, the SDK will be ready for Uniswap V4 launch, enabling protocols to integrate V4 pools immediately upon mainnet launch.

---

**Repository**: [https://github.com/diegomig/MIG_SDK_EXPORT](https://github.com/diegomig/MIG_SDK_EXPORT)  
**License**: MIT OR Apache-2.0 (Open Source)  
**Focus**: Uniswap V2/V3/V4 on Arbitrum  
**V4 Status**: Preparation (this grant), Integration (post-launch)
