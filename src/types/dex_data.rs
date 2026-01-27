use crate::types::conversions::{u256_to_decimal, ConversionError};
use ethers::types::{Address, U256};
use rust_decimal::Decimal;

// Dato crudo de ethers-rs (lo que recibes)
#[derive(Debug, Clone)]
pub struct RawUniswapV3Pool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub liquidity: u128,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub tick_spacing: i32,
}

// Tu tipo homogenizado (lo que usas internamente)
#[derive(Debug, Clone)]
pub struct NormalizedPool {
    pub pool_address: String,
    pub token0_address: String,
    pub token1_address: String,
    pub fee_bps: u16,   // En basis points
    pub price: Decimal, // Precio real token1/token0
    pub liquidity_usd: Decimal,
    pub dex_type: DexType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DexType {
    UniswapV2,
    UniswapV3,
    Sushiswap,
    Camelot,
}

// Conversión V3
impl TryFrom<RawUniswapV3Pool> for NormalizedPool {
    type Error = ConversionError;

    fn try_from(raw: RawUniswapV3Pool) -> Result<Self, Self::Error> {
        // sqrtPriceX96 a precio real
        let sqrt_price = u256_to_decimal(raw.sqrt_price_x96, 0)?;
        // Usar aproximación matemática más simple para evitar problemas con powd
        let price = sqrt_price * sqrt_price / Decimal::from(2u128.pow(192));

        Ok(NormalizedPool {
            pool_address: crate::types::conversions::address_to_string(raw.address),
            token0_address: crate::types::conversions::address_to_string(raw.token0),
            token1_address: crate::types::conversions::address_to_string(raw.token1),
            fee_bps: (raw.fee / 100) as u16,
            price,
            liquidity_usd: Decimal::ZERO, // Calcular después
            dex_type: DexType::UniswapV3,
        })
    }
}
