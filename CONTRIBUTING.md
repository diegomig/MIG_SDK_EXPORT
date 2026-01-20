# Contributing to MIG Topology SDK

Thank you for your interest in contributing to the MIG Topology SDK! This document provides guidelines and standards for contributing to the project.

## Code of Conduct

- Be respectful and inclusive
- Focus on constructive feedback
- Help maintain a welcoming environment for all contributors

## Development Setup

### Prerequisites

- Rust 1.75+ (stable)
- PostgreSQL 14+
- Redis 7+ (optional, for caching features)
- Local Arbitrum node (optional, for development)

### Getting Started

1. **Clone the repository:**
   ```bash
   git clone https://github.com/mig-labs/mig-topology-sdk.git
   cd mig-topology-sdk
   ```

2. **Set up environment:**
   ```bash
   cp .env.example .env
   # Edit .env with your configuration
   ```

3. **Run tests:**
   ```bash
   cargo test
   ```

4. **Check code quality:**
   ```bash
   cargo clippy --all-targets -- -D warnings
   cargo fmt --check
   ```

## Coding Standards

### Rust Style Guide

We follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) and use `rustfmt` for formatting.

**Formatting:**
```bash
cargo fmt
```

**Linting:**
```bash
cargo clippy --all-targets -- -D warnings
```

### Code Style Requirements

1. **Use `rustfmt`**: All code must be formatted with `rustfmt`
2. **Pass `clippy`**: All code must pass `clippy` with no warnings
3. **Documentation**: Public APIs must have rustdoc comments (`///`)
4. **Error Handling**: Use `anyhow::Result` for general errors, `thiserror` for structured errors
5. **Naming**: Follow Rust naming conventions (snake_case for functions, PascalCase for types)

### Example: Well-Formatted Code

```rust
/// Fetches pool state for the given pool metadata.
///
/// # Arguments
///
/// * `pool_meta` - Pool metadata containing address and token information
/// * `block` - Optional block number for historical queries
///
/// # Returns
///
/// Pool state with reserves/liquidity, or an error if fetch fails
pub async fn fetch_pool_state(
    &self,
    pool_meta: &PoolMeta,
    block: Option<BlockId>,
) -> Result<Pool> {
    // Implementation
}
```

## Testing Standards

### Test Organization

Tests are organized into three categories:

1. **Unit Tests**: Test individual functions and modules
2. **Integration Tests**: Test component interactions
3. **Property-Based Tests**: Test invariants and edge cases

### Unit Tests

Unit tests live alongside the code they test in `#[cfg(test)]` modules:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_validation() {
        // Test implementation
    }
}
```

### Integration Tests

Integration tests live in `tests/` directory:

```rust
// tests/integration_test.rs
use mig_topology_sdk::*;

#[tokio::test]
async fn test_discovery_cycle() {
    // Test implementation
}
```

### Property-Based Testing

We use `proptest` for property-based testing of critical algorithms:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_weight_calculation_invariants(
        reserve0 in 0u128..u128::MAX,
        reserve1 in 0u128..u128::MAX,
    ) {
        // Test invariants
    }
}
```

### Test Coverage Goals

- **Unit Tests**: >80% coverage for core modules
- **Integration Tests**: Cover all public APIs
- **Property-Based Tests**: Critical algorithms (weight calculation, normalization)

### Running Tests

```bash
# All tests
cargo test

# Unit tests only
cargo test --lib

# Integration tests only
cargo test --test '*'

# With coverage (requires cargo-tarpaulin)
cargo tarpaulin --out Html
```

## Documentation Standards

### Rustdoc Requirements

All public APIs must have comprehensive rustdoc comments:

```rust
/// Calculates the liquidity weight for a pool in USD.
///
/// The weight represents the total value locked (TVL) in the pool,
/// calculated as the sum of token reserves multiplied by their USD prices.
///
/// # Arguments
///
/// * `pool` - The pool to calculate weight for
/// * `prices` - USD prices for tokens in the pool
///
/// # Returns
///
/// The liquidity weight in USD, or an error if calculation fails
///
/// # Example
///
/// ```rust
/// let weight = graph_service.calculate_weight(&pool, &prices)?;
/// println!("Pool weight: ${:.2}", weight);
/// ```
pub fn calculate_weight(&self, pool: &Pool, prices: &HashMap<Address, f64>) -> Result<f64> {
    // Implementation
}
```

### Module Documentation

Each module should have a module-level doc comment:

```rust
//! # Pool Validator Module
//!
//! The `PoolValidator` performs quality assurance checks on discovered pools
//! to ensure only legitimate, high-quality pools are included in the topology graph.
//!
//! ## Validation Criteria
//!
//! - Bytecode verification
//! - Token validation
//! - Liquidity checks
//!
//! See `docs/VALIDATION.md` for detailed validation criteria.
```

### Example Code

All examples in documentation must be compilable and tested:

```bash
# Test examples
cargo test --doc
cargo run --example basic_setup
```

## Pull Request Process

### Before Submitting

1. **Update Documentation**: Ensure all public APIs are documented
2. **Add Tests**: Include tests for new functionality
3. **Run Checks**: Ensure `cargo fmt`, `cargo clippy`, and `cargo test` all pass
4. **Update CHANGELOG**: Add entry describing your changes

### PR Checklist

- [ ] Code follows style guidelines (`cargo fmt`, `cargo clippy`)
- [ ] Tests added/updated and passing
- [ ] Documentation updated (rustdoc comments)
- [ ] CHANGELOG.md updated
- [ ] No breaking changes (or breaking changes documented)

### PR Description Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Testing
Describe how you tested your changes

## Checklist
- [ ] Code follows style guidelines
- [ ] Tests added/updated
- [ ] Documentation updated
```

## Adding New DEX Protocols

To add support for a new DEX protocol:

1. **Create Adapter**: Implement `DexAdapter` trait in `src/adapters/`
2. **Add Pool Type**: Add enum variant to `Pool` if needed
3. **Update Normalization**: Add conversion logic in `normalization.rs`
4. **Add Tests**: Include unit and integration tests
5. **Update Documentation**: Document protocol-specific behavior

See `docs/ARCHITECTURE.md` for detailed instructions.

## Performance Considerations

### Optimization Guidelines

1. **Profile First**: Use `cargo flamegraph` or `perf` to identify bottlenecks
2. **Measure Impact**: Benchmark before and after optimizations
3. **Document Trade-offs**: Explain performance vs. complexity trade-offs

### Performance Targets

- **Discovery Latency**: <2s per block
- **State Fetch Latency**: <100ms (local node: <10ms)
- **Graph Update Latency**: <500ms for 2,000 pools
- **Memory Usage**: <1MB per 1,000 pools

## Error Handling

### Error Types

- **`anyhow::Result`**: For general error propagation with context
- **`thiserror`**: For structured, matchable error types

### Error Context

Always provide context for errors:

```rust
// Good
anyhow::bail!("Failed to fetch pool state: {}", e)

// Better
anyhow::bail!("Failed to fetch pool state for {:?}: {}", pool_address, e)
```

## Security Considerations

### Security Guidelines

1. **Input Validation**: Validate all external inputs (addresses, block numbers)
2. **No Private Keys**: SDK is read-only, never store or use private keys
3. **Rate Limiting**: Respect RPC provider rate limits
4. **Error Messages**: Don't leak sensitive information in error messages

### Reporting Security Issues

Please report security issues privately to security@mig-labs.com

## Questions?

- **Documentation**: See `docs/` directory
- **Architecture**: See `docs/ARCHITECTURE.md`
- **Examples**: See `examples/` directory
- **Issues**: Open an issue on GitHub

Thank you for contributing to MIG Topology SDK! 🚀

