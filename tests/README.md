# Integration Tests for P0/P1 Optimizations

This directory contains integration tests for the P0 and P1 optimizations implemented in the MIG Topology SDK.

## Test Files

- **`test_p1_optimizations.rs`**: Tests for batch database updates
- **`test_cache_optimizations.rs`**: Tests for cache invalidation and TTL differentiation
- **`test_parallel_price_fetch.rs`**: Tests for parallel price fetching configuration

## Running Tests

### All Tests
```bash
cargo test
```

### Specific Test File
```bash
cargo test --test test_p1_optimizations
cargo test --test test_cache_optimizations
cargo test --test test_parallel_price_fetch
```

### Tests Requiring Database
Some tests require a database connection. To run them:
```bash
cargo test --test test_p1_optimizations -- --ignored
```

Make sure `DATABASE_URL` is set in your environment or `.env` file.

## Test Coverage

### P0 Optimizations (Completed)
- ✅ Merkle tree-based cache invalidation
- ✅ TTL differentiation (touched vs untouched pools)
- ✅ Fuzzy block matching
- ⚠️ Local node tests excluded (as per requirements)

### P1 Optimizations (Completed)
- ✅ Batch database updates
- ✅ Parallel price fetching configuration
- ✅ Settings validation

## Notes

- **Local Node Tests**: Excluded as per requirements (no local node available)
- **Database Tests**: Marked with `#[ignore]` - run with `--ignored` flag
- **Mock Tests**: Some tests use mocks/simulations to avoid requiring full infrastructure
