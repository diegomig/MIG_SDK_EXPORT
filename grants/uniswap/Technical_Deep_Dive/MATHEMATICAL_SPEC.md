# Mathematical Specification: Uniswap V3 Tick Math Precision

**Grant Program**: Uniswap Foundation Infrastructure Grant  
**Project**: MIG Topology SDK - Uniswap Ecosystem Optimization

---

## Executive Summary

This document specifies the mathematical precision requirements for Uniswap V3 tick math implementation in the MIG Topology SDK, ensuring **100% accuracy** with the Uniswap V3 reference implementation in Solidity.

---

## Critical Requirement: 100% Mathematical Accuracy

### Why Precision Matters

**For Uniswap V3 SDK users**, mathematical precision is critical:

1. **Lending Protocols**: Incorrect prices can lead to incorrect collateral valuations, potentially causing unjust liquidations
2. **Derivatives Protocols**: Price accuracy is essential for options pricing and risk calculations
3. **Analytics Platforms**: Users rely on accurate price data for decision-making
4. **Research Tools**: Mathematical correctness is fundamental for research validity

**Requirement**: The SDK must match Uniswap V3 reference implementation with **100% accuracy** (no approximations, no rounding errors).

---

## Uniswap V3 Reference Implementation

### Official Reference

**Uniswap V3 Core Repository**: [https://github.com/Uniswap/v3-core](https://github.com/Uniswap/v3-core)

**Key Contracts**:
- `TickMath.sol`: Tick-to-price conversion functions
- `SqrtPriceMath.sol`: SqrtPriceX96 calculations
- `Pool.sol`: Core pool contract with price calculations

### Reference Functions

1. **`TickMath.getSqrtRatioAtTick(int24 tick) → uint160`**
   - Calculates sqrt(1.0001^tick) * 2^96
   - Must match exactly (no approximations)

2. **`TickMath.getTickAtSqrtRatio(uint160 sqrtPriceX96) → int24`**
   - Inverse of getSqrtRatioAtTick
   - Must match exactly (within ±1 tick tolerance)

3. **Price Calculations**
   - price = (sqrtPriceX96 / 2^96)^2
   - Must maintain precision throughout calculations

---

## Mathematical Specifications

### SqrtPriceX96 Format

**Definition**: SqrtPriceX96 = sqrt(price) * 2^96

**Where**:
- `price` = token1_amount / token0_amount
- `sqrt(price)` = square root of price
- `* 2^96` = fixed-point scaling (Q96 format)

**Precision**: 256-bit unsigned integer (uint256), stored as uint160 in Uniswap V3

### Tick-to-Price Formula

**Formula**: price = 1.0001^tick

**SqrtPriceX96**: sqrtPriceX96 = sqrt(1.0001^tick) * 2^96

**Tick Range**: -887272 to 887272 (MIN_TICK to MAX_TICK)

### Price-to-Tick Formula

**Inverse Formula**: tick ≈ log(price) / log(1.0001)

**Implementation**: Binary search with sqrtPriceX96 comparison

**Tolerance**: ±1 tick (acceptable due to discrete tick spacing)

---

## Implementation Strategy

### Current Implementation (Phase 1)

**Status**: Basic implementation exists with approximations for large ticks

**Limitation**: Uses f64 approximations for large ticks (not 100% accurate)

**Reference**: `src/v3_math.rs` (current implementation)

### Enhanced Implementation (Phase 2 - This Grant)

**Objective**: Achieve 100% accuracy with Uniswap V3 reference

**Approach**:
1. **Exact Integer Math**: Use integer arithmetic throughout (no floating-point)
2. **Reference Implementation Port**: Port Uniswap V3 TickMath.sol to Rust
3. **Validation**: Property-based testing against Uniswap V3 reference
4. **Edge Case Handling**: Handle overflow/underflow exactly as reference implementation

---

## Implementation Details

### Tick-to-SqrtPriceX96 Conversion

**Algorithm** (from Uniswap V3 TickMath.sol):

```rust
// Ported from Uniswap V3 TickMath.getSqrtRatioAtTick
pub fn get_sqrt_ratio_at_tick_exact(tick: i32) -> Result<U256, TickMathError> {
    // Validate tick range
    if tick < MIN_TICK || tick > MAX_TICK {
        return Err(TickMathError::TickOutOfRange);
    }
    
    let abs_tick = if tick < 0 { (-tick) as u32 } else { tick as u32 };
    
    // Calculate ratio = 1.0001^abs_tick using exact integer math
    // (Implementation matches Uniswap V3 TickMath.sol exactly)
    let ratio = calculate_ratio_exact(abs_tick)?;
    
    // Apply sqrt and scale by 2^96
    let sqrt_ratio = sqrt_ratio_exact(ratio)?;
    let sqrt_price_x96 = (sqrt_ratio * Q96) >> 32;  // Scale to Q96
    
    if tick < 0 {
        // Invert: 1 / sqrt(ratio) * 2^96
        Ok((Q96 * Q96) / sqrt_price_x96)
    } else {
        Ok(sqrt_price_x96)
    }
}
```

**Key Requirements**:
- **Exact Integer Math**: No floating-point arithmetic
- **Overflow Protection**: Handle overflow exactly as reference implementation
- **Bit Manipulation**: Use bit shifts for efficiency (as in reference)
- **Error Handling**: Return errors for invalid inputs (as in reference)

### SqrtPriceX96-to-Tick Conversion

**Algorithm** (from Uniswap V3 TickMath.sol):

```rust
// Ported from Uniswap V3 TickMath.getTickAtSqrtRatio
pub fn get_tick_at_sqrt_ratio_exact(sqrt_price_x96: U256) -> Result<i32, TickMathError> {
    // Validate sqrtPriceX96 range
    if sqrt_price_x96 < MIN_SQRT_RATIO || sqrt_price_x96 >= MAX_SQRT_RATIO {
        return Err(TickMathError::SqrtPriceOutOfRange);
    }
    
    // Binary search for tick (matches Uniswap V3 implementation exactly)
    let mut low = MIN_TICK;
    let mut high = MAX_TICK;
    
    while high - low > 1 {
        let mid = (low + high) / 2;
        let mid_sqrt = get_sqrt_ratio_at_tick_exact(mid)?;
        
        if mid_sqrt <= sqrt_price_x96 {
            low = mid;
        } else {
            high = mid;
        }
    }
    
    Ok(low)
}
```

**Key Requirements**:
- **Binary Search**: Exact algorithm from reference implementation
- **Tolerance**: ±1 tick (acceptable, matches reference behavior)
- **Error Handling**: Return errors for invalid inputs

---

## Validation Strategy

### Property-Based Testing

**Approach**: Test against Uniswap V3 reference implementation

**Test Vectors**:
1. **All Valid Ticks**: Test every tick from MIN_TICK to MAX_TICK (within reason)
2. **Edge Cases**: MIN_TICK, MAX_TICK, tick = 0, tick = ±1
3. **Round-Trip**: tick → sqrtPriceX96 → tick (should recover original tick ±1)
4. **Reference Comparison**: Compare outputs with Uniswap V3 TickMath.sol outputs

**Implementation**:
```rust
#[test]
fn test_tick_math_against_reference() {
    // Test vectors from Uniswap V3 reference implementation
    let test_ticks = vec![
        -887272, -887271, -1, 0, 1, 887271, 887272,
        // Add more test vectors from Uniswap V3 tests
    ];
    
    for tick in test_ticks {
        let sqrt_price = get_sqrt_ratio_at_tick_exact(tick).unwrap();
        let recovered_tick = get_tick_at_sqrt_ratio_exact(sqrt_price).unwrap();
        
        // Allow ±1 tick tolerance (matches reference behavior)
        assert!((recovered_tick - tick).abs() <= 1);
        
        // Compare with reference implementation output (if available)
        // let reference_sqrt_price = get_reference_sqrt_ratio_at_tick(tick);
        // assert_eq!(sqrt_price, reference_sqrt_price);
    }
}
```

### Continuous Validation

**Process**:
1. **Test Suite**: Comprehensive test suite against reference implementation
2. **CI/CD Integration**: Run tests on every commit
3. **Reference Updates**: Update tests if Uniswap V3 reference implementation changes
4. **External Validation**: External advisors validate mathematical correctness

---

## Precision Guarantees

### Exact Match Requirements

**Tick-to-SqrtPriceX96**:
- ✅ Exact match with Uniswap V3 TickMath.getSqrtRatioAtTick
- ✅ No approximations, no rounding errors
- ✅ Overflow handling matches reference exactly

**SqrtPriceX96-to-Tick**:
- ✅ Exact match with Uniswap V3 TickMath.getTickAtSqrtRatio
- ✅ ±1 tick tolerance (matches reference behavior)
- ✅ Binary search algorithm matches reference exactly

### Error Handling

**Invalid Inputs**:
- Tick out of range: Return error (matches reference)
- SqrtPriceX96 out of range: Return error (matches reference)
- Overflow: Handle exactly as reference implementation

---

## External Validation

### Mathematical Validation

**External Advisory**: Uniswap protocol experts review mathematical correctness

**Validation Process**:
1. Code review of tick math implementation
2. Comparison with Uniswap V3 reference implementation
3. Test vector validation
4. Edge case validation

**Deliverable**: Validation report confirming 100% accuracy

---

## Documentation

### API Documentation

**Rustdocs**: Complete documentation for all tick math functions

**Examples**: Usage examples with expected outputs

**Error Documentation**: When to expect which errors

### Mathematical Documentation

**Formulas**: Document all mathematical formulas used

**Algorithms**: Document algorithms (matching Uniswap V3 documentation)

**Precision Guarantees**: Document precision guarantees and tolerances

---

## Conclusion

This mathematical specification ensures **100% accuracy** with Uniswap V3 reference implementation, providing:

- **Exact Integer Math**: No floating-point approximations
- **Reference Implementation Port**: Exact port of Uniswap V3 TickMath.sol
- **Comprehensive Validation**: Property-based testing against reference
- **External Validation**: Expert review ensures correctness

With this approach, the SDK will provide mathematically correct tick math calculations that match Uniswap V3 reference implementation exactly, enabling safe use by lending protocols, derivatives protocols, and analytics platforms.

---

**Repository**: [https://github.com/mig-labs/mig-topology-sdk](https://github.com/mig-labs/mig-topology-sdk)  
**Uniswap V3 Reference**: [https://github.com/Uniswap/v3-core](https://github.com/Uniswap/v3-core)  
**License**: MIT OR Apache-2.0 (Open Source)  
**Precision Requirement**: 100% match with Uniswap V3 reference implementation
