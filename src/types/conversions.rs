use ethers::types::{Address, I256, U256};
use rust_decimal::Decimal;
use std::str::FromStr;

// Para precios y cantidades (crítico en HFT)
pub fn u256_to_decimal(value: U256, decimals: u8) -> Result<Decimal, ConversionError> {
    let value_str = value.to_string();
    let decimal_value = Decimal::from_str(&value_str)
        .map_err(|e| ConversionError::InvalidDecimal(e.to_string()))?;

    // Ajustar por decimales del token
    let divisor = Decimal::from(10u128.pow(decimals as u32));
    Ok(decimal_value / divisor)
}

// Para ticks de V3 (son I24, vienen como I256)
pub fn i256_to_i32(value: I256) -> Result<i32, ConversionError> {
    let as_i128: i128 = value.try_into().map_err(|_| ConversionError::Overflow)?;

    i32::try_from(as_i128).map_err(|_| ConversionError::Overflow)
}

// Para addresses
pub fn address_to_string(addr: Address) -> String {
    format!("{:?}", addr).to_lowercase()
}

pub fn string_to_address(s: &str) -> Result<Address, ConversionError> {
    Address::from_str(s).map_err(|e| ConversionError::InvalidAddress(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("Invalid decimal: {0}")]
    InvalidDecimal(String),
    #[error("Overflow in conversion")]
    Overflow,
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
}
