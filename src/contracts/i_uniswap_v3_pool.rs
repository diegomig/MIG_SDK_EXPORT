use ethers::prelude::*;

// ═══════════════════════════════════════════════════════════════════════════
// UNISWAP V3 POOL ABI - OFICIAL Y COMPLETO
// ═══════════════════════════════════════════════════════════════════════════
//
// IMPORTANTE: Este ABI usa los tipos EXACTOS del contrato de Solidity
// - uint160 para sqrtPriceX96 (NO uint256)
// - int24 para tick (NO int256 ni int32)
// - uint24 para fee (NO uint256)
// - uint128 para liquidity (NO uint256)
//
// Cualquier desviación causará errores de decodificación silenciosos
// ═══════════════════════════════════════════════════════════════════════════

abigen!(
    IUniswapV3Pool,
    r#"[
        {
            "inputs": [],
            "name": "slot0",
            "outputs": [
                {
                    "internalType": "uint160",
                    "name": "sqrtPriceX96",
                    "type": "uint160"
                },
                {
                    "internalType": "int24",
                    "name": "tick",
                    "type": "int24"
                },
                {
                    "internalType": "uint16",
                    "name": "observationIndex",
                    "type": "uint16"
                },
                {
                    "internalType": "uint16",
                    "name": "observationCardinality",
                    "type": "uint16"
                },
                {
                    "internalType": "uint16",
                    "name": "observationCardinalityNext",
                    "type": "uint16"
                },
                {
                    "internalType": "uint8",
                    "name": "feeProtocol",
                    "type": "uint8"
                },
                {
                    "internalType": "bool",
                    "name": "unlocked",
                    "type": "bool"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "liquidity",
            "outputs": [
                {
                    "internalType": "uint128",
                    "name": "",
                    "type": "uint128"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "fee",
            "outputs": [
                {
                    "internalType": "uint24",
                    "name": "",
                    "type": "uint24"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "token0",
            "outputs": [
                {
                    "internalType": "address",
                    "name": "",
                    "type": "address"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "token1",
            "outputs": [
                {
                    "internalType": "address",
                    "name": "",
                    "type": "address"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "tickSpacing",
            "outputs": [
                {
                    "internalType": "int24",
                    "name": "",
                    "type": "int24"
                }
            ],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#
);

// ═══════════════════════════════════════════════════════════════════════════
// VALIDACIÓN DE DATOS V3
// ═══════════════════════════════════════════════════════════════════════════

use ethers::types::{Address, U256};

pub const MIN_TICK: i32 = -887272;
pub const MAX_TICK: i32 = 887272;
pub const VALID_FEE_TIERS: [u32; 4] = [100, 500, 3000, 10000];

pub fn validate_slot0(
    sqrt_price_x96: U256,
    tick: i32,
    pool_address: &Address,
) -> Result<(), String> {
    // Validar sqrt_price_x96
    if sqrt_price_x96.is_zero() {
        return Err(format!(
            "Invalid sqrt_price_x96: zero value for pool {}",
            pool_address
        ));
    }

    // Validar tick
    if tick < MIN_TICK || tick > MAX_TICK {
        return Err(format!(
            "Invalid tick: {} not in range [{}, {}] for pool {}",
            tick, MIN_TICK, MAX_TICK, pool_address
        ));
    }

    Ok(())
}

pub fn validate_fee_tier(fee: u32, pool_address: &Address) -> Result<(), String> {
    if !VALID_FEE_TIERS.contains(&fee) {
        return Err(format!(
            "Invalid fee tier: {} not in {:?} for pool {}",
            fee, VALID_FEE_TIERS, pool_address
        ));
    }
    Ok(())
}

pub fn validate_liquidity(liquidity: u128, pool_address: &Address) -> Result<(), String> {
    if liquidity == 0 {
        return Err(format!(
            "Invalid liquidity: zero value for pool {}",
            pool_address
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Address;

    #[test]
    fn test_validate_slot0() {
        let pool_addr = Address::zero();
        let valid_sqrt_price = U256::from(1000);
        let valid_tick = 0;

        assert!(validate_slot0(valid_sqrt_price, valid_tick, &pool_addr).is_ok());
        assert!(validate_slot0(U256::zero(), valid_tick, &pool_addr).is_err());
        assert!(validate_slot0(valid_sqrt_price, MIN_TICK - 1, &pool_addr).is_err());
        assert!(validate_slot0(valid_sqrt_price, MAX_TICK + 1, &pool_addr).is_err());
    }

    #[test]
    fn test_validate_fee_tier() {
        let pool_addr = Address::zero();

        assert!(validate_fee_tier(500, &pool_addr).is_ok());
        assert!(validate_fee_tier(3000, &pool_addr).is_ok());
        assert!(validate_fee_tier(10000, &pool_addr).is_ok());
        assert!(validate_fee_tier(100, &pool_addr).is_ok());

        assert!(validate_fee_tier(2500, &pool_addr).is_err());
        assert!(validate_fee_tier(30, &pool_addr).is_err());
    }

    #[test]
    fn test_validate_liquidity() {
        let pool_addr = Address::zero();

        assert!(validate_liquidity(1000, &pool_addr).is_ok());
        assert!(validate_liquidity(0, &pool_addr).is_err());
    }
}
