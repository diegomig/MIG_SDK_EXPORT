// src/normalization.rs
//
// Fixed-point normalization utilities for converting token amounts between different tokens
// with correct decimal handling and price ratio application.

use anyhow::Result;
use ethers::types::{Address, U256};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Fixed-point scale (1e18) for preserving precision in integer arithmetic
pub const SCALE: u128 = 1_000_000_000_000_000_000u128;

/// Safe multiply then divide: (a * b) / denom with U256, returning floor.
/// Handles overflow by saturating multiplication.
#[inline]
pub fn mul_div_floor(a: U256, b: U256, denom: U256) -> U256 {
    if a.is_zero() || b.is_zero() {
        return U256::zero();
    }
    if denom.is_zero() {
        return U256::zero(); // Avoid division by zero
    }
    let prod = a.saturating_mul(b);
    prod / denom
}

/// Convert amount_out (in token_exit base units) to entry-token base units:
///
/// Formula: amount_out_in_entry = amount_out * exit_per_entry_fixed * 10^(dec_entry - dec_exit)
///
/// Parameters:
/// - amount_out: The output amount in exit token's base units (wei-like)
/// - exit_per_entry_fixed: Fixed-point ratio (scaled by SCALE=1e18) representing how many
///   entry token units one exit token unit is worth. I.e., price of exit in terms of entry.
/// - dec_out: Decimals of the exit token (e.g., 6 for USDC, 18 for WETH)
/// - dec_entry: Decimals of the entry token
///
/// Returns: Amount in entry token base units, or None if calculation fails
#[inline(never)] // Large function, don't inline to avoid code bloat
pub fn convert_amount_out_to_entry(
    amount_out: U256,
    exit_per_entry_fixed: U256,
    dec_out: u8,
    dec_entry: u8,
) -> Option<U256> {
    if amount_out.is_zero() || exit_per_entry_fixed.is_zero() {
        return Some(U256::zero());
    }

    // Get decimal adjustment factors
    let pow_entry = pow10_u128(dec_entry)?;
    let pow_out = pow10_u128(dec_out)?;

    // Formula breakdown:
    // amount_out_in_entry = amount_out * exit_per_entry_fixed * 10^dec_entry / (SCALE * 10^dec_out)
    //
    // Numerator = amount_out * exit_per_entry_fixed * pow_entry
    // Denominator = SCALE * pow_out

    // Step 1: amount_out * exit_per_entry_fixed
    let numerator = mul_div_floor(amount_out, exit_per_entry_fixed, U256::one());

    // Step 2: multiply by pow_entry
    let numerator = numerator.saturating_mul(U256::from(pow_entry));

    // Step 3: divide by (SCALE * pow_out)
    let denom = U256::from(SCALE).saturating_mul(U256::from(pow_out));

    Some(numerator / denom)
}

/// Helper: 10^n as u128, returns None if overflow (n > ~38)
#[inline]
pub fn pow10_u128(n: u8) -> Option<u128> {
    match n {
        0 => Some(1u128),
        1 => Some(10u128),
        2 => Some(100u128),
        3 => Some(1_000u128),
        4 => Some(10_000u128),
        5 => Some(100_000u128),
        6 => Some(1_000_000u128),
        7 => Some(10_000_000u128),
        8 => Some(100_000_000u128),
        9 => Some(1_000_000_000u128),
        10 => Some(10_000_000_000u128),
        11 => Some(100_000_000_000u128),
        12 => Some(1_000_000_000_000u128),
        13 => Some(10_000_000_000_000u128),
        14 => Some(100_000_000_000_000u128),
        15 => Some(1_000_000_000_000_000u128),
        16 => Some(10_000_000_000_000_000u128),
        17 => Some(100_000_000_000_000_000u128),
        18 => Some(1_000_000_000_000_000_000u128),
        19 => Some(10_000_000_000_000_000_000u128),
        20 => Some(100_000_000_000_000_000_000u128),
        21 => Some(1_000_000_000_000_000_000_000u128),
        22 => Some(10_000_000_000_000_000_000_000u128),
        23 => Some(100_000_000_000_000_000_000_000u128),
        24 => Some(1_000_000_000_000_000_000_000_000u128),
        25 => Some(10_000_000_000_000_000_000_000_000u128),
        26 => Some(100_000_000_000_000_000_000_000_000u128),
        27 => Some(1_000_000_000_000_000_000_000_000_000u128),
        _ => None, // Beyond u128 capacity
    }
}

/// Decimal helper: 10^n as Decimal (n <= 38)
fn pow10_decimal(n: u8) -> Option<Decimal> {
    pow10_u128(n).map(Decimal::from)
}

/// Normalize a base-unit amount (U256) into human-decimal Decimal using token decimals
pub fn normalize_amount(amount: U256, decimals: u8) -> Decimal {
    let scale = match pow10_decimal(decimals) {
        Some(s) => s,
        None => return Decimal::ZERO,
    };
    let amt_dec = Decimal::from_str(&amount.to_string()).unwrap_or(Decimal::ZERO);
    amt_dec / scale
}

/// Denormalize a human Decimal back into base units (U256) using token decimals (floor)
pub fn denormalize_amount(value: Decimal, decimals: u8) -> U256 {
    let scale = match pow10_decimal(decimals) {
        Some(s) => s,
        None => return U256::zero(),
    };
    // Multiply and floor by truncation via to_string integer extraction
    let scaled = value * scale;
    // Ensure non-negative and integer by truncating fractional part
    let s = scaled.trunc().to_string();
    U256::from_dec_str(&s).unwrap_or_else(|_| U256::zero())
}

/// Strongly-typed normalized amount in base units (hot path uses U256)
#[derive(Clone, Debug)]
pub struct NormalizedAmount {
    pub token: Address,
    pub decimals: u8,
    pub amount_base_units: U256,
}

/// Parse a human amount string (e.g. "123.4567") into base units (U256) using token decimals.
/// Intended for off-path inputs; hot path should already operate in base units.
pub fn parse_human_amount_to_u256(amount_str: &str, decimals: u8) -> Result<U256> {
    let value = Decimal::from_str(amount_str)?;
    Ok(denormalize_amount(value, decimals))
}

/// Normalize price ratio num/den considering token decimals
/// Returns Decimal in human scale (no additional scaling)
pub fn normalize_price(num: U256, den: U256, num_dec: u8, den_dec: u8) -> Option<Decimal> {
    if den.is_zero() {
        return None;
    }
    let num_h = normalize_amount(num, num_dec);
    let den_h = normalize_amount(den, den_dec);
    if den_h.is_zero() {
        return None;
    }
    Some(num_h / den_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pow10_basic() {
        assert_eq!(pow10_u128(0), Some(1));
        assert_eq!(pow10_u128(6), Some(1_000_000));
        assert_eq!(pow10_u128(18), Some(1_000_000_000_000_000_000));
    }

    #[test]
    fn test_convert_6_to_18() {
        // 1,000,000 USDC (6 decimals) -> WETH (18 decimals)
        // Assuming 1:1 price ratio
        let amount_out = U256::from(1_000_000u64); // 1 USDC in base units
        let exit_per_entry_fixed = U256::from(SCALE); // 1:1 ratio

        let result =
            convert_amount_out_to_entry(amount_out, exit_per_entry_fixed, 6u8, 18u8).unwrap();

        // Expected: 1_000_000 * 10^(18-6) = 1_000_000 * 1e12 = 1e18
        let expected = U256::from(1_000_000u128) * U256::from(1_000_000_000_000u128);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_convert_18_to_6() {
        // 1e18 base units WETH (18 decimals) -> USDC (6 decimals)
        // Assuming 1:1 price ratio
        let amount_out = U256::from(1_000_000_000_000_000_000u128); // 1 WETH
        let exit_per_entry_fixed = U256::from(SCALE); // 1:1 ratio

        let result =
            convert_amount_out_to_entry(amount_out, exit_per_entry_fixed, 18u8, 6u8).unwrap();

        // Expected: 1e18 * 10^(6-18) = 1e18 / 1e12 = 1e6
        assert_eq!(result, U256::from(1_000_000u64));
    }

    #[test]
    fn test_convert_18_to_18() {
        // 2 WETH (18 decimals) -> DAI (18 decimals)
        // Assuming 1:1 price ratio
        let amount_out = U256::from(2_000_000_000_000_000_000u128); // 2 tokens
        let exit_per_entry_fixed = U256::from(SCALE); // 1:1 ratio

        let result =
            convert_amount_out_to_entry(amount_out, exit_per_entry_fixed, 18u8, 18u8).unwrap();

        // Expected: same amount since same decimals and 1:1 ratio
        assert_eq!(result, amount_out);
    }

    #[test]
    fn test_price_ratio_application() {
        // 2 units of exit token (18 decimals), price = 2.0 (exit costs 2x entry)
        // So 2 units exit = 4 units entry
        let amount_out = U256::from(2_000_000_000_000_000_000u128); // 2 tokens
        let exit_per_entry_fixed = U256::from(SCALE * 2u128); // 2:1 ratio (exit is 2x more valuable)

        let result =
            convert_amount_out_to_entry(amount_out, exit_per_entry_fixed, 18u8, 18u8).unwrap();

        // Expected: 2 * 2 = 4 tokens in entry units
        assert_eq!(result, U256::from(4_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_zero_handling() {
        let exit_per_entry_fixed = U256::from(SCALE);

        // Zero amount_out
        let result =
            convert_amount_out_to_entry(U256::zero(), exit_per_entry_fixed, 18u8, 18u8).unwrap();
        assert_eq!(result, U256::zero());

        // Zero ratio
        let amount_out = U256::from(1_000_000u64);
        let result = convert_amount_out_to_entry(amount_out, U256::zero(), 18u8, 18u8).unwrap();
        assert_eq!(result, U256::zero());
    }

    #[test]
    fn test_mul_div_floor() {
        // Simple test: (100 * 50) / 10 = 500
        let a = U256::from(100u64);
        let b = U256::from(50u64);
        let denom = U256::from(10u64);
        assert_eq!(mul_div_floor(a, b, denom), U256::from(500u64));

        // Division by zero safety
        assert_eq!(mul_div_floor(a, b, U256::zero()), U256::zero());
    }
}
