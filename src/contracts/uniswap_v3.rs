use ethers::abi::{decode, ParamType, Token};
use ethers::prelude::abigen;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, Bytes, U256};
use std::sync::Arc;

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
    UniswapV3Pool,
    r#"[
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)
        function liquidity() external view returns (uint128)
        function token0() external view returns (address)
        function token1() external view returns (address)
        function fee() external view returns (uint24)
        function tickSpacing() external view returns (int24)
        function observe(uint32[] calldata secondsAgos) external view returns (int56[] tickCumulatives, uint160[] secondsPerLiquidityCumulativeX128s)
    ]"#
);

// Uniswap V3 Quoter (para simular swaps)
abigen!(
    UniswapV3Quoter,
    r#"[
        function quoteExactInputSingle(address tokenIn, address tokenOut, uint24 fee, uint256 amountIn, uint160 sqrtPriceLimitX96) external returns (uint256 amountOut)
    ]"#
);

// ═══════════════════════════════════════════════════════════════════════════
// WRAPPER PARA MANEJAR int24 CORRECTAMENTE
// ═══════════════════════════════════════════════════════════════════════════

/// Wrapper para decodificar slot0 con manejo correcto de int24
pub async fn fetch_slot0_safe<M: ethers::providers::Middleware + 'static>(
    pool_address: ethers::types::Address,
    provider: std::sync::Arc<M>,
) -> Result<Slot0Data, Box<dyn std::error::Error>> {
    let pool = UniswapV3Pool::new(pool_address, provider.clone());

    // Llamar slot0 (el macro genera slot_0)
    let raw_result = pool.slot_0().call().await?;

    eprintln!("🔍 Raw slot0 result: {:?}", raw_result);

    // Decodificar manualmente con validación
    let (sqrt_price_x96, tick_raw, obs_idx, obs_card, obs_card_next, fee_protocol, unlocked) =
        raw_result;

    // CRÍTICO: Convertir tick de forma segura
    // El abigen genera esto como i32, pero Solidity lo retorna como int24
    // Necesitamos validar que está en el rango de int24

    let tick = if tick_raw >= -8388608 && tick_raw <= 8388607 {
        // Rango válido de int24: -2^23 a 2^23-1
        tick_raw
    } else {
        eprintln!(
            "⚠️  Warning: tick {} outside int24 range, clamping",
            tick_raw
        );
        tick_raw.clamp(-8388608, 8388607)
    };

    // Validar que tick está en el rango de Uniswap V3
    if tick < -887272 || tick > 887272 {
        return Err(format!(
            "Tick {} outside Uniswap V3 valid range [-887272, 887272]",
            tick
        )
        .into());
    }

    Ok(Slot0Data {
        sqrt_price_x96,
        tick,
        observation_index: obs_idx,
        observation_cardinality: obs_card,
        observation_cardinality_next: obs_card_next,
        fee_protocol,
        unlocked,
    })
}

#[derive(Debug, Clone)]
pub struct Slot0Data {
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub observation_index: u16,
    pub observation_cardinality: u16,
    pub observation_cardinality_next: u16,
    pub fee_protocol: u8,
    pub unlocked: bool,
}

/// Decodificar slot0 manualmente sin abigen
pub async fn fetch_slot0_manual<M: ethers::providers::Middleware>(
    pool_address: ethers::types::Address,
    provider: std::sync::Arc<M>,
) -> Result<Slot0Data, Box<dyn std::error::Error>>
where
    <M as ethers::providers::Middleware>::Error: 'static,
{
    // Crear la llamada raw a slot0()
    let slot0_selector = [0x38, 0x50, 0xc7, 0xbd]; // keccak256("slot0()")[0..4]

    let call_data = ethers::types::transaction::eip2718::TypedTransaction::Legacy(
        ethers::types::TransactionRequest {
            to: Some(ethers::types::NameOrAddress::Address(pool_address)),
            data: Some(ethers::types::Bytes::from(slot0_selector.to_vec())),
            ..Default::default()
        },
    );

    eprintln!("📞 Calling slot0() on {:?}", pool_address);
    let result = provider.call(&call_data, None).await?;

    eprintln!(
        "📦 Raw response ({} bytes): 0x{}",
        result.len(),
        hex::encode(&result)
    );

    // Decodificar manualmente
    let decoded = decode(
        &[
            ParamType::Uint(160), // sqrtPriceX96
            ParamType::Int(24),   // tick ← AQUÍ especificamos int24 explícitamente
            ParamType::Uint(16),  // observationIndex
            ParamType::Uint(16),  // observationCardinality
            ParamType::Uint(16),  // observationCardinalityNext
            ParamType::Uint(8),   // feeProtocol
            ParamType::Bool,      // unlocked
        ],
        &result,
    )?;

    eprintln!("✅ Decoded {} tokens", decoded.len());

    // Extraer valores
    let sqrt_price_x96 = match &decoded[0] {
        Token::Uint(val) => *val,
        _ => return Err("Invalid sqrtPriceX96".into()),
    };

    let tick = match &decoded[1] {
        Token::Int(val) => {
            // Convertir de U256 a i32 con manejo de signo
            let tick_u64 = val.as_u64();
            eprintln!("🔢 Decoded tick: {}", tick_u64);

            // Convertir u64 a i32 con manejo de signo
            let tick = if tick_u64 & 0x80000000 != 0 {
                // Número negativo
                (tick_u64 as i64 - 0x100000000i64) as i32
            } else {
                tick_u64 as i32
            };

            if tick < -887272 || tick > 887272 {
                return Err(format!("Tick {} out of range", tick).into());
            }

            tick
        }
        _ => return Err("Invalid tick".into()),
    };

    let observation_index = match &decoded[2] {
        Token::Uint(val) => val.as_u64() as u16,
        _ => return Err("Invalid observationIndex".into()),
    };

    let observation_cardinality = match &decoded[3] {
        Token::Uint(val) => val.as_u64() as u16,
        _ => return Err("Invalid observationCardinality".into()),
    };

    let observation_cardinality_next = match &decoded[4] {
        Token::Uint(val) => val.as_u64() as u16,
        _ => return Err("Invalid observationCardinalityNext".into()),
    };

    let fee_protocol = match &decoded[5] {
        Token::Uint(val) => val.as_u64() as u8,
        _ => return Err("Invalid feeProtocol".into()),
    };

    let unlocked = match &decoded[6] {
        Token::Bool(val) => *val,
        _ => return Err("Invalid unlocked".into()),
    };

    Ok(Slot0Data {
        sqrt_price_x96,
        tick,
        observation_index,
        observation_cardinality,
        observation_cardinality_next,
        fee_protocol,
        unlocked,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// VALIDADORES DE DATOS V3
// ═══════════════════════════════════════════════════════════════════════════

/// Rangos válidos para tick en Uniswap V3
pub const MIN_TICK: i32 = -887272;
pub const MAX_TICK: i32 = 887272;

/// Fee tiers válidos en Uniswap V3 (en basis points)
pub const VALID_FEE_TIERS: [u32; 4] = [100, 500, 3000, 10000];

/// Validar datos de slot0
pub fn validate_slot0(
    sqrt_price_x96: U256,
    tick: i32,
    pool_address: &Address,
) -> Result<(), String> {
    // Validar sqrt_price
    if sqrt_price_x96.is_zero() {
        return Err(format!("Pool {}: Invalid sqrt_price_x96 = 0", pool_address));
    }

    // Validar que sqrt_price está en rango razonable
    // MIN: sqrt(1.0001^-887272) * 2^96 ≈ 4295128739
    // MAX: sqrt(1.0001^887272) * 2^96 ≈ 1.461e45
    let min_sqrt_price = U256::from(4295128739u64);
    let max_sqrt_price =
        U256::from_dec_str("1461446703485210103287273052203988822378723970342").unwrap();

    if sqrt_price_x96 < min_sqrt_price || sqrt_price_x96 > max_sqrt_price {
        return Err(format!(
            "Pool {}: sqrt_price_x96 {} out of valid range [{}, {}]",
            pool_address, sqrt_price_x96, min_sqrt_price, max_sqrt_price
        ));
    }

    // Validar tick
    if tick < MIN_TICK || tick > MAX_TICK {
        return Err(format!(
            "Pool {}: tick {} out of valid range [{}, {}]",
            pool_address, tick, MIN_TICK, MAX_TICK
        ));
    }

    Ok(())
}

/// Validar fee tier
pub fn validate_fee_tier(fee: u32, pool_address: &Address) -> Result<(), String> {
    if !VALID_FEE_TIERS.contains(&fee) {
        return Err(format!(
            "Pool {}: Invalid fee tier {}. Valid: {:?}",
            pool_address, fee, VALID_FEE_TIERS
        ));
    }
    Ok(())
}

/// Validar liquidity
pub fn validate_liquidity(liquidity: u128, pool_address: &Address) -> Result<(), String> {
    if liquidity == 0 {
        return Err(format!(
            "Pool {}: Zero liquidity (inactive or empty pool)",
            pool_address
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_tick() {
        let addr = Address::zero();
        let valid_sqrt_price = U256::from_dec_str("79228162514264337593543950336").unwrap(); // Corresponds to a tick of 0 for price 1

        // Valid ticks
        assert!(validate_slot0(valid_sqrt_price, 0, &addr).is_ok());
        assert!(validate_slot0(valid_sqrt_price, MIN_TICK, &addr).is_ok());
        assert!(validate_slot0(valid_sqrt_price, MAX_TICK, &addr).is_ok());

        // Invalid ticks
        assert!(validate_slot0(valid_sqrt_price, MIN_TICK - 1, &addr).is_err());
        assert!(validate_slot0(valid_sqrt_price, MAX_TICK + 1, &addr).is_err());

        // Invalid sqrt_price
        assert!(validate_slot0(U256::zero(), 0, &addr).is_err());
    }

    #[test]
    fn test_validate_fee_tier() {
        let addr = Address::zero();

        // Valid fees
        for fee in VALID_FEE_TIERS.iter() {
            assert!(validate_fee_tier(*fee, &addr).is_ok());
        }

        // Invalid fees
        assert!(validate_fee_tier(30, &addr).is_err());
        assert!(validate_fee_tier(2500, &addr).is_err());
        assert!(validate_fee_tier(0, &addr).is_err());
    }

    #[test]
    fn test_validate_liquidity() {
        let addr = Address::zero();

        // Valid liquidity
        assert!(validate_liquidity(1000, &addr).is_ok());
        assert!(validate_liquidity(u128::MAX, &addr).is_ok());

        // Invalid liquidity
        assert!(validate_liquidity(0, &addr).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DECODIFICACIÓN RAW DE SLOT0() - SIN ABIGEN
// ═══════════════════════════════════════════════════════════════════════════

/// Decodificar slot0() manualmente usando llamada raw con selector 0x3850c7bd
/// Esta función evita completamente abigen para el campo tick (int24)
pub async fn fetch_slot0_raw(
    pool_address: Address,
    provider: Arc<Provider<Http>>,
) -> Result<(U256, i32, u16, u16, u16, u8, bool), Box<dyn std::error::Error>> {
    // Selector de slot0(): keccak256("slot0()")[0..4] = 0x3850c7bd
    let slot0_selector = [0x38, 0x50, 0xc7, 0xbd];

    let call_data = ethers::types::transaction::eip2718::TypedTransaction::Legacy(
        ethers::types::TransactionRequest {
            to: Some(ethers::types::NameOrAddress::Address(pool_address)),
            data: Some(Bytes::from(slot0_selector.to_vec())),
            ..Default::default()
        },
    );

    println!("📞 Calling slot0() raw on {:?}", pool_address);
    let raw_result = provider.call(&call_data, None).await?;

    println!(
        "📦 Raw response ({} bytes): 0x{}",
        raw_result.len(),
        hex::encode(&raw_result)
    );

    // Verificar que tenemos suficientes bytes (mínimo 32 bytes para slot0)
    if raw_result.len() < 32 {
        return Err("Invalid slot0 response: insufficient data".into());
    }

    // Decodificar manualmente byte por byte
    let raw_bytes = raw_result.as_ref();

    // uint160 sqrtPriceX96 (bytes 0-31) - 32 bytes con padding ABI estándar
    let sqrt_price_bytes: [u8; 32] = raw_bytes[0..32].try_into().unwrap();
    let sqrt_price_x96 = U256::from_big_endian(&sqrt_price_bytes);

    // int24 tick (bytes 32-35) - 4 bytes signed (ABI padding)
    let tick_bytes: [u8; 4] = raw_bytes[32..36].try_into().unwrap();
    let tick_i32 = i32::from_be_bytes(tick_bytes);

    // uint16 observationIndex (bytes 36-37)
    let obs_idx_bytes: [u8; 2] = raw_bytes[36..38].try_into().unwrap();
    let observation_index = u16::from_be_bytes(obs_idx_bytes);

    // uint16 observationCardinality (bytes 38-39)
    let obs_card_bytes: [u8; 2] = raw_bytes[38..40].try_into().unwrap();
    let observation_cardinality = u16::from_be_bytes(obs_card_bytes);

    // uint16 observationCardinalityNext (bytes 40-41)
    let obs_card_next_bytes: [u8; 2] = raw_bytes[40..42].try_into().unwrap();
    let observation_cardinality_next = u16::from_be_bytes(obs_card_next_bytes);

    // uint8 feeProtocol (byte 42)
    let fee_protocol = raw_bytes[42];

    // bool unlocked (byte 43)
    let unlocked = raw_bytes[43] != 0;

    println!("🔢 Decoded tick raw: {}", tick_i32);

    // Validar que tick está en el rango de Uniswap V3
    if tick_i32 < -887272 || tick_i32 > 887272 {
        return Err(format!(
            "Tick {} outside Uniswap V3 valid range [-887272, 887272]",
            tick_i32
        )
        .into());
    }

    Ok((
        sqrt_price_x96,
        tick_i32,
        observation_index,
        observation_cardinality,
        observation_cardinality_next,
        fee_protocol,
        unlocked,
    ))
}
