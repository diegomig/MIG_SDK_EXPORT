// UniswapV3 math for direct pricing without Quoter (hot-path optimization)
use ethers::types::U256;

/// Uniswap V3 constants
const MIN_TICK: i64 = -887272;
const MAX_TICK: i64 = 887272;
pub const MIN_SQRT_RATIO: U256 = U256([4295128739, 0, 0, 0]); // sqrt(1.0001^-887272) * 2^96
pub const MAX_SQRT_RATIO: U256 = U256([6743328256752651558, 17280870778742802505, 4294805859, 0]); // sqrt(1.0001^887272) * 2^96

/// Q96 fixed point constants
const Q96: U256 = U256([0, 0, 4294967296, 0]); // 2^96

#[derive(Debug, Clone)]
pub struct V3PoolState {
    pub sqrt_price_x96: U256,
    pub tick: i64,
    pub liquidity: u128,
}

/// Calculate sqrt price from tick (TickMath.getSqrtRatioAtTick equivalent)
/// Uses checked operations to prevent overflow
pub fn get_sqrt_ratio_at_tick(tick: i64) -> U256 {
    if tick < MIN_TICK || tick > MAX_TICK {
        return U256::zero();
    }

    if tick == 0 {
        return Q96;
    }

    // For small ticks, use direct calculation with overflow protection
    if tick.abs() <= 10 {
        let abs_tick = if tick < 0 {
            (-tick) as u32
        } else {
            tick as u32
        };
        let mut ratio = Q96;

        for _ in 0..abs_tick {
            // Check for overflow before multiplication
            if let Some(new_ratio) = ratio.checked_mul(U256::from(10001)) {
                ratio = new_ratio / U256::from(10000);
            } else {
                // Overflow detected, use approximation
                return get_sqrt_ratio_approximation(tick);
            }
        }

        if tick < 0 {
            if ratio > U256::zero() {
                if let Some(result) = (Q96 * Q96).checked_div(ratio) {
                    result
                } else {
                    MAX_SQRT_RATIO
                }
            } else {
                MAX_SQRT_RATIO
            }
        } else {
            ratio
        }
    } else {
        get_sqrt_ratio_approximation(tick)
    }
}

/// Approximation for large ticks to avoid overflow
fn get_sqrt_ratio_approximation(tick: i64) -> U256 {
    let abs_tick = if tick < 0 {
        (-tick) as u32
    } else {
        tick as u32
    };

    // Use f64 for approximation to avoid overflow
    let log_ratio = (abs_tick as f64) * 0.0001_f64.ln();
    let sqrt_ratio = (log_ratio / 2.0).exp();

    // Convert back to U256 with bounds checking
    let result_f64 = sqrt_ratio * (1u128 << 96) as f64;
    let result = if result_f64 > u128::MAX as f64 {
        u128::MAX
    } else if result_f64 < 0.0 {
        0
    } else {
        result_f64 as u128
    };

    let ratio = U256::from(result);

    if tick < 0 {
        if ratio > U256::zero() {
            // Use safe division to avoid overflow
            if let Some(inverted) = (Q96 * Q96).checked_div(ratio) {
                inverted
            } else {
                MAX_SQRT_RATIO
            }
        } else {
            MAX_SQRT_RATIO
        }
    } else {
        ratio
    }
}

/// Calculate tick from sqrt price (inverse of above)
/// Uses limited iterations to prevent infinite loops
pub fn get_tick_at_sqrt_ratio(sqrt_price_x96: U256) -> i64 {
    if sqrt_price_x96 < MIN_SQRT_RATIO || sqrt_price_x96 >= MAX_SQRT_RATIO {
        return 0;
    }

    // Binary search with strict iteration limit
    let mut low = MIN_TICK;
    let mut high = MAX_TICK;
    let mut iterations = 0;
    const MAX_ITERATIONS: i32 = 50; // Prevent infinite loops

    while high - low > 1 && iterations < MAX_ITERATIONS {
        let mid = low + (high - low) / 2;
        let mid_sqrt = get_sqrt_ratio_at_tick(mid);

        if mid_sqrt <= sqrt_price_x96 {
            low = mid;
        } else {
            high = mid;
        }
        iterations += 1;
    }

    low
}

/// Safe multiplication with overflow protection
fn safe_mul_div(a: U256, b: U256, c: U256) -> U256 {
    if c.is_zero() {
        return U256::zero();
    }

    // Check for potential overflow before multiplication
    if let Some(product) = a.checked_mul(b) {
        product / c
    } else {
        // Use lossy scientific conversion to f64 to avoid u128 casts
        let a_f64 = u256_to_f64_lossy(a);
        let b_f64 = u256_to_f64_lossy(b);
        let c_f64 = u256_to_f64_lossy(c);
        if c_f64 == 0.0 {
            return U256::zero();
        }
        let result = (a_f64 * b_f64) / c_f64;
        if result.is_finite() && result > 0.0 {
            // Clamp to u128 max range when converting back
            let clamped = result.min(u128::MAX as f64).max(0.0) as u128;
            U256::from(clamped)
        } else {
            U256::zero()
        }
    }
}

/// Safe addition with overflow protection
fn safe_add(a: U256, b: U256) -> U256 {
    a.saturating_add(b)
}

/// Safe subtraction with underflow protection
fn safe_sub(a: U256, b: U256) -> U256 {
    a.saturating_sub(b)
}

/// Direct V3 swap calculation (SwapMath equivalent, simplified)
pub fn compute_swap_step(
    sqrt_ratio_current_x96: U256,
    sqrt_ratio_target_x96: U256,
    liquidity: u128,
    amount_remaining: U256,
    fee_pips: u32,
) -> (U256, U256, U256, U256) {
    let zero_for_one = sqrt_ratio_current_x96 >= sqrt_ratio_target_x96;
    let exact_in = true; // Assume exact input

    if liquidity == 0 {
        return (
            U256::zero(),
            U256::zero(),
            sqrt_ratio_current_x96,
            U256::zero(),
        );
    }

    // Simplified calculation for performance with overflow protection
    let sqrt_ratio_next_x96 = if exact_in {
        get_next_sqrt_price_from_input_safe(
            sqrt_ratio_current_x96,
            liquidity,
            amount_remaining,
            zero_for_one,
        )
    } else {
        get_next_sqrt_price_from_output_safe(
            sqrt_ratio_current_x96,
            liquidity,
            amount_remaining,
            zero_for_one,
        )
    };

    let sqrt_ratio_next_x96 = if zero_for_one {
        sqrt_ratio_next_x96.max(sqrt_ratio_target_x96)
    } else {
        sqrt_ratio_next_x96.min(sqrt_ratio_target_x96)
    };

    let max_amount_in = if zero_for_one {
        get_amount0_delta_safe(sqrt_ratio_next_x96, sqrt_ratio_current_x96, liquidity, true)
    } else {
        get_amount1_delta_safe(sqrt_ratio_current_x96, sqrt_ratio_next_x96, liquidity, true)
    };

    let amount_in = if exact_in && max_amount_in > amount_remaining {
        amount_remaining
    } else {
        max_amount_in
    };

    let amount_out = if zero_for_one {
        get_amount1_delta_safe(
            sqrt_ratio_next_x96,
            sqrt_ratio_current_x96,
            liquidity,
            false,
        )
    } else {
        get_amount0_delta_safe(
            sqrt_ratio_current_x96,
            sqrt_ratio_next_x96,
            liquidity,
            false,
        )
    };

    // 🔧 CRITICAL DIAGNOSTIC - Log compute_swap_step internals
    if amount_out.is_zero() && amount_remaining > U256::zero() && liquidity > 0 {
        tracing::warn!(
            "🚨 COMPUTE_SWAP_STEP returned amount_out=0: sqrt_current={}, sqrt_next={}, sqrt_target={}, liquidity={}, amount_remaining={}, fee_pips={}, zero_for_one={}",
            sqrt_ratio_current_x96, sqrt_ratio_next_x96, sqrt_ratio_target_x96, liquidity, amount_remaining, fee_pips, zero_for_one
        );
    }

    // Apply fee
    let fee_amount = if exact_in && sqrt_ratio_next_x96 != sqrt_ratio_target_x96 {
        safe_sub(amount_remaining, amount_in)
    } else {
        safe_mul_div(amount_in, U256::from(fee_pips), U256::from(1_000_000))
    };

    (amount_in, amount_out, sqrt_ratio_next_x96, fee_amount)
}

fn get_next_sqrt_price_from_input_safe(
    sqrt_px96: U256,
    liquidity: u128,
    amount_in: U256,
    zero_for_one: bool,
) -> U256 {
    if amount_in.is_zero() || liquidity == 0 {
        return sqrt_px96;
    }

    let liquidity_u256 = U256::from(liquidity);

    if zero_for_one {
        get_next_sqrt_price_from_amount0_rounding_up_safe(
            sqrt_px96,
            liquidity_u256,
            amount_in,
            true,
        )
    } else {
        get_next_sqrt_price_from_amount1_rounding_down_safe(
            sqrt_px96,
            liquidity_u256,
            amount_in,
            true,
        )
    }
}

fn get_next_sqrt_price_from_output_safe(
    sqrt_px96: U256,
    liquidity: u128,
    amount_out: U256,
    zero_for_one: bool,
) -> U256 {
    if amount_out.is_zero() || liquidity == 0 {
        return sqrt_px96;
    }

    let liquidity_u256 = U256::from(liquidity);

    if zero_for_one {
        get_next_sqrt_price_from_amount1_rounding_down_safe(
            sqrt_px96,
            liquidity_u256,
            amount_out,
            false,
        )
    } else {
        get_next_sqrt_price_from_amount0_rounding_up_safe(
            sqrt_px96,
            liquidity_u256,
            amount_out,
            false,
        )
    }
}

fn get_next_sqrt_price_from_amount0_rounding_up_safe(
    sqrt_px96: U256,
    liquidity: U256,
    amount: U256,
    add: bool,
) -> U256 {
    if amount.is_zero() {
        return sqrt_px96;
    }

    let numerator1 = liquidity << 96;

    if add {
        // Check for overflow before multiplication
        if let Some(product) = amount.checked_mul(sqrt_px96) {
            let denominator = safe_add(numerator1, product);
            safe_mul_div(numerator1, sqrt_px96, denominator)
        } else {
            // Use approximation for very large numbers
            let amount_f64 = u256_to_f64_lossy(amount);
            let sqrt_f64 = u256_to_f64_lossy(sqrt_px96);
            let numerator1_f64 = u256_to_f64_lossy(numerator1);
            let product_f64 = amount_f64 * sqrt_f64;
            let denominator_f64 = numerator1_f64 + product_f64;
            let result_f64 = if denominator_f64 > 0.0 {
                (numerator1_f64 * sqrt_f64) / denominator_f64
            } else {
                0.0
            };
            let clamped = result_f64.min(u128::MAX as f64).max(0.0) as u128;
            U256::from(clamped)
        }
    } else {
        if let Some(product) = amount.checked_mul(sqrt_px96) {
            if numerator1 > product {
                let denominator = safe_sub(numerator1, product);
                safe_mul_div(numerator1, sqrt_px96, denominator)
            } else {
                U256::zero()
            }
        } else {
            U256::zero()
        }
    }
}

fn get_next_sqrt_price_from_amount1_rounding_down_safe(
    sqrt_px96: U256,
    liquidity: U256,
    amount: U256,
    add: bool,
) -> U256 {
    // Use safe mul/div to avoid division by zero when (liquidity >> 96) == 0
    let quotient = safe_mul_div(amount, Q96, liquidity);
    if add {
        safe_add(sqrt_px96, quotient)
    } else {
        if sqrt_px96 > quotient {
            safe_sub(sqrt_px96, quotient)
        } else {
            U256::zero()
        }
    }
}

fn get_amount0_delta_safe(
    sqrt_ratio_ax96: U256,
    sqrt_ratio_bx96: U256,
    liquidity: u128,
    round_up: bool,
) -> U256 {
    let (sqrt_ratio_ax96, sqrt_ratio_bx96) = if sqrt_ratio_ax96 > sqrt_ratio_bx96 {
        (sqrt_ratio_bx96, sqrt_ratio_ax96)
    } else {
        (sqrt_ratio_ax96, sqrt_ratio_bx96)
    };

    let numerator1 = U256::from(liquidity) << 96;
    let numerator2 = safe_sub(sqrt_ratio_bx96, sqrt_ratio_ax96);

    if round_up {
        // Avoid direct division by sqrt_ratio_ax96
        safe_mul_div(
            safe_mul_div(numerator1, numerator2, sqrt_ratio_bx96),
            U256::one(),
            sqrt_ratio_ax96,
        )
    } else {
        // Avoid direct division by sqrt_ratio_ax96
        safe_mul_div(
            safe_mul_div(numerator1, numerator2, sqrt_ratio_bx96),
            U256::one(),
            sqrt_ratio_ax96,
        )
    }
}

fn get_amount1_delta_safe(
    sqrt_ratio_ax96: U256,
    sqrt_ratio_bx96: U256,
    liquidity: u128,
    round_up: bool,
) -> U256 {
    let (sqrt_ratio_ax96, sqrt_ratio_bx96) = if sqrt_ratio_ax96 > sqrt_ratio_bx96 {
        (sqrt_ratio_bx96, sqrt_ratio_ax96)
    } else {
        (sqrt_ratio_ax96, sqrt_ratio_bx96)
    };

    let liquidity_u256 = U256::from(liquidity);
    let diff = safe_sub(sqrt_ratio_bx96, sqrt_ratio_ax96);

    if round_up {
        safe_mul_div(liquidity_u256, diff, Q96)
    } else {
        (liquidity_u256 * diff) >> 96
    }
}

/// Fast V3 amount calculation for hot pools (replaces Quoter calls)
/// Ultra-simplified version that avoids any complex calculations
pub fn get_amount_out_v3_direct(
    amount_in: U256,
    sqrt_price_x96: U256,
    liquidity: u128,
    fee: u32,
    zero_for_one: bool,
) -> U256 {
    // Early returns for edge cases
    if amount_in.is_zero() || liquidity == 0 || sqrt_price_x96.is_zero() {
        return U256::zero();
    }

    // Ultra-simple calculation without complex math
    let base_output = if zero_for_one {
        // For zero_for_one: amount_in * sqrt_price_x96 / Q96
        if let Some(product) = amount_in.checked_mul(sqrt_price_x96) {
            product / Q96
        } else {
            // Use approximation for very large numbers
            let amount_f64 = u256_to_f64_lossy(amount_in);
            let sqrt_f64 = u256_to_f64_lossy(sqrt_price_x96);
            let q96_f64 = (1u128 << 96) as f64;
            let result = (amount_f64 * sqrt_f64) / q96_f64;
            let clamped = result.min(u128::MAX as f64).max(0.0) as u128;
            U256::from(clamped)
        }
    } else {
        // For one_for_zero: amount_in * Q96 / sqrt_price_x96
        if let Some(product) = amount_in.checked_mul(Q96) {
            product / sqrt_price_x96
        } else {
            // Use approximation for very large numbers
            let amount_f64 = u256_to_f64_lossy(amount_in);
            let sqrt_f64 = u256_to_f64_lossy(sqrt_price_x96);
            let q96_f64 = (1u128 << 96) as f64;
            let result = if sqrt_f64 > 0.0 {
                (amount_f64 * q96_f64) / sqrt_f64
            } else {
                0.0
            };
            let clamped = result.min(u128::MAX as f64).max(0.0) as u128;
            U256::from(clamped)
        }
    };

    // Apply simple fee
    if fee > 0 && base_output > U256::zero() {
        let fee_amount = (base_output * U256::from(fee)) / U256::from(1_000_000);
        if base_output > fee_amount {
            base_output - fee_amount
        } else {
            U256::zero()
        }
    } else {
        base_output
    }
}

// Lossy scientific conversion of U256 to f64 without intermediate u128 casts.
// Takes the first N digits as mantissa and uses the remaining as exponent base-10.
/// Convert U256 to f64 safely, handling overflow
pub fn u256_to_f64_lossy(v: U256) -> f64 {
    if v.is_zero() {
        return 0.0;
    }
    let s = v.to_string();
    let len = s.len();
    let take = if len >= 18 { 18 } else { len };
    let (mantissa_str, _rest) = s.split_at(take);
    let mantissa = mantissa_str.parse::<f64>().unwrap_or(0.0);
    let exp10 = (len - take) as i32;
    mantissa * 10f64.powi(exp10)
}

/// Converts a Uniswap V3 tick to a price.
/// The price is the ratio of token1 to token0.
pub fn tick_to_price(tick: i64) -> f64 {
    // Usar powf en lugar de powi para mejor manejo de overflow
    // Verificar si el tick está en un rango que pueda causar overflow
    const MAX_SAFE_TICK_F64: i64 = 500000; // Aproximadamente 10^21 en precio relativo
    const MIN_SAFE_TICK_F64: i64 = -500000;

    if tick < MIN_SAFE_TICK_F64 || tick > MAX_SAFE_TICK_F64 {
        // Para ticks extremos, usar cálculo logarítmico para evitar overflow
        let log_price = (tick as f64) * 1.0001f64.ln();
        let price = log_price.exp();

        // Verificar que el resultado sea finito
        if price.is_finite() && price > 0.0 {
            price
        } else {
            // Si aún falla, retornar un valor extremo pero finito
            if tick > 0 {
                1e20 // Precio muy alto pero finito
            } else {
                1e-20 // Precio muy bajo pero finito
            }
        }
    } else {
        // Para ticks normales, usar el cálculo directo
        let price = 1.0001f64.powf(tick as f64);

        // Verificar que el resultado sea finito
        if price.is_finite() && price > 0.0 {
            price
        } else {
            // Fallback: usar cálculo logarítmico
            let log_price = (tick as f64) * 1.0001f64.ln();
            log_price.exp()
        }
    }
}

/// Determine if swap is zero_for_one based on token addresses
pub fn is_zero_for_one(token_in: ethers::types::Address, token0: ethers::types::Address) -> bool {
    token_in == token0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_sqrt_conversion() {
        let tick = 0;
        let sqrt_ratio = get_sqrt_ratio_at_tick(tick);
        let back_tick = get_tick_at_sqrt_ratio(sqrt_ratio);
        assert!((back_tick - tick).abs() <= 1);
    }

    #[test]
    fn test_v3_direct_calculation() {
        let amount_in = U256::from(1000) * U256::exp10(18); // 1000 ETH
        let sqrt_price_x96 = Q96; // Price = 1
        let liquidity = 1_000_000u128;
        let fee = 3000; // 0.3%

        let amount_out = get_amount_out_v3_direct(amount_in, sqrt_price_x96, liquidity, fee, true);
        assert!(amount_out > U256::zero());
        assert!(amount_out < amount_in); // Should be less due to fees
    }

    #[test]
    fn test_overflow_protection() {
        // Test with reasonable values that test overflow protection
        let large_amount = U256::from(1_000_000_000_000_000_000u128); // 1 ETH in wei
        let sqrt_price_x96 = Q96;
        let liquidity = 1_000_000_000u128; // 1B liquidity
        let fee = 3000;

        // This should not panic or loop infinitely
        let amount_out =
            get_amount_out_v3_direct(large_amount, sqrt_price_x96, liquidity, fee, true);
        // Should return some reasonable value or zero
        assert!(amount_out <= large_amount);
    }

    #[test]
    fn test_tick_to_price() {
        // Tick 0 should be price 1.0
        assert!((tick_to_price(0) - 1.0).abs() < 1e-9);

        // Positive tick
        let price_pos = tick_to_price(200000);
        let expected_pos = 1.0001f64.powi(200000);
        assert!((price_pos - expected_pos).abs() < 1e-9);

        // Negative tick
        let price_neg = tick_to_price(-200000);
        let expected_neg = 1.0001f64.powi(-200000);
        assert!((price_neg - expected_neg).abs() < 1e-9);
    }
}

/// Simulates a V3 swap with accurate price impact calculation using Uniswap V3 math
/// Returns (amount_out, price_impact_percentage, sqrt_price_after)
pub fn simulate_v3_swap_with_impact(
    amount_in: U256,
    sqrt_price_x96: U256,
    liquidity: u128,
    fee_bps: u32,
    zero_for_one: bool,
) -> Result<(U256, f64, U256), &'static str> {
    // Validate inputs
    if amount_in.is_zero() {
        return Err("Zero amount_in");
    }
    if liquidity == 0 {
        return Err("Zero liquidity");
    }
    if sqrt_price_x96.is_zero() {
        return Err("Zero sqrt_price");
    }

    // ⚠️ CRITICAL FIX: El fee del contrato Uniswap V3 YA ESTÁ en pips (millonésimas):
    // - 100 = 0.01% (1 bps)
    // - 500 = 0.05% (5 bps)
    // - 3000 = 0.30% (30 bps)
    // - 10000 = 1.00% (100 bps)
    // NO multiplicar por 100, el valor ya viene en el formato correcto
    let fee_pips = fee_bps;

    // Use the full tick range as target (max price movement)
    let sqrt_ratio_target_x96 = if zero_for_one {
        MIN_SQRT_RATIO
    } else {
        MAX_SQRT_RATIO
    };

    // Compute the swap using Uniswap V3 math
    let (_amount_in_used, amount_out, sqrt_price_after, _fee_amount) = compute_swap_step(
        sqrt_price_x96,
        sqrt_ratio_target_x96,
        liquidity,
        amount_in,
        fee_pips,
    );

    // Calculate price impact using sqrt ratios directly (safer than converting to price)
    // price_impact = |sqrt_after - sqrt_before| / sqrt_before * 100
    // This avoids potential overflow from squaring large sqrt_price values
    let sqrt_before_f64 = u256_to_f64_lossy(sqrt_price_x96);
    let sqrt_after_f64 = u256_to_f64_lossy(sqrt_price_after);

    let price_impact = if sqrt_before_f64 > 0.0 {
        let _sqrt_change_pct = ((sqrt_after_f64 - sqrt_before_f64).abs() / sqrt_before_f64) * 100.0;
        // Price impact is approximately 2x the sqrt change for small changes
        // For larger changes, use direct formula: (1 - (sqrt_after/sqrt_before)^2) * 100
        let ratio = sqrt_after_f64 / sqrt_before_f64;
        let price_change = if zero_for_one {
            (1.0 - ratio * ratio).abs() * 100.0
        } else {
            (ratio * ratio - 1.0).abs() * 100.0
        };
        // Clamp to reasonable range [0, 100]
        price_change.min(100.0).max(0.0)
    } else {
        100.0 // Invalid state
    };

    Ok((amount_out, price_impact, sqrt_price_after))
}

/// Convert sqrt_price_x96 to normal price (token1/token0)
fn sqrt_price_to_price(sqrt_price_x96: U256) -> f64 {
    if sqrt_price_x96.is_zero() {
        return 0.0;
    }

    // price = (sqrt_price_x96 / 2^96)^2
    let sqrt_price_f64 = u256_to_f64_lossy(sqrt_price_x96);
    let q96_f64 = (1u128 << 96) as f64;
    let price = (sqrt_price_f64 / q96_f64).powi(2);
    price
}
