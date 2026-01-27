//! 🔧 DATA PIPELINE - Centralizado, auditable y consistente
//!
//! Este módulo implementa un pipeline único y auditable para todos los datos
//! que entran al bot. Procesa datos crudos de RPC/Subgraph/Multicall siguiendo
//! el flujo: Parsing → Normalización → Validación → Serialización final.
//!
//! Principios:
//! - Un solo punto de entrada para todos los datos crudos
//! - Conversión explícita y validada
//! - Logs estructurados con tracing
//! - Tests unitarios para cada conversión
//! - Metadatos de origen en todas las structs

use anyhow::{anyhow, Context, Result};
use ethers::prelude::*;
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, instrument, warn};

/// 🎯 TIPOS DE FUENTES DE DATOS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataSource {
    MulticalV1,
    MulticalV2,
    RpcDirect,
    SubgraphUniswapV2,
    SubgraphUniswapV3,
    SubgraphCamelot,
    Cache,
    Simulation,
}

impl std::fmt::Display for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            DataSource::MulticalV1 => "MulticallV1",
            DataSource::MulticalV2 => "MulticallV2",
            DataSource::RpcDirect => "RPC_Direct",
            DataSource::SubgraphUniswapV2 => "Subgraph_UniV2",
            DataSource::SubgraphUniswapV3 => "Subgraph_UniV3",
            DataSource::SubgraphCamelot => "Subgraph_Camelot",
            DataSource::Cache => "Cache",
            DataSource::Simulation => "Simulation",
        };
        write!(f, "{}", name)
    }
}

/// 📊 METADATOS DE ORIGEN PARA TODOS LOS DATOS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMetadata {
    pub source: DataSource,
    pub timestamp_ms: u64,
    pub block_number: Option<u64>,
    pub raw_input: Option<String>, // Para debugging
    pub parsing_errors: Vec<String>,
}

/// 🎯 RESULTADO DE CONVERSIÓN CON METADATOS
#[derive(Debug, Clone)]
pub struct ConversionResult<T> {
    pub data: T,
    pub metadata: DataMetadata,
    pub is_valid: bool,
    pub validation_errors: Vec<String>,
}

/// 🏗️ TRAIT PARA TIPOS QUE PUEDEN SER CONVERTIDOS
pub trait DataConvertible: Sized {
    type RawInput;

    /// Convierte datos crudos a struct tipada con validación
    fn from_raw(raw: Self::RawInput, source: DataSource) -> Result<ConversionResult<Self>>;

    /// Valida la consistencia interna de los datos
    fn validate_internal_consistency(&self) -> Vec<String>;
}

/// 🧮 CONVERSIÓN HEX A U256 CON VALIDACIÓN
#[instrument(skip(raw_hex), fields(input = ?raw_hex))]
pub fn parse_hex_to_u256(raw_hex: &str) -> Result<ConversionResult<U256>> {
    debug!("Parsing hex to U256");

    let metadata = DataMetadata {
        source: DataSource::MulticalV1, // Default, será sobreescrito
        timestamp_ms: current_timestamp_ms(),
        block_number: None,
        raw_input: Some(raw_hex.to_string()),
        parsing_errors: Vec::new(),
    };

    // Remover 0x prefix si existe
    let hex_str = raw_hex.strip_prefix("0x").unwrap_or(raw_hex);

    match U256::from_str_radix(hex_str, 16) {
        Ok(value) => {
            let mut result = ConversionResult {
                data: value,
                metadata,
                is_valid: true,
                validation_errors: Vec::new(),
            };

            // Validar que no sea un valor obviamente inválido
            if value == U256::zero() {
                result
                    .validation_errors
                    .push("U256 value is zero".to_string());
                result.is_valid = false;
            }

            debug!(result = ?value, valid = result.is_valid, "Hex to U256 conversion complete");
            Ok(result)
        }
        Err(e) => {
            error!(error = ?e, "Failed to parse hex to U256");
            let mut metadata = metadata;
            metadata
                .parsing_errors
                .push(format!("Hex parsing error: {}", e));

            Ok(ConversionResult {
                data: U256::zero(),
                metadata,
                is_valid: false,
                validation_errors: vec![format!("Hex parsing failed: {}", e)],
            })
        }
    }
}

/// 🏠 CONVERSIÓN ADDRESS CON CHECKSUM
#[instrument(skip(raw_address), fields(input = ?raw_address))]
pub fn parse_address_to_h160(raw_address: &str) -> Result<ConversionResult<Address>> {
    debug!("Parsing address to H160");

    let metadata = DataMetadata {
        source: DataSource::MulticalV1,
        timestamp_ms: current_timestamp_ms(),
        block_number: None,
        raw_input: Some(raw_address.to_string()),
        parsing_errors: Vec::new(),
    };

    match raw_address.parse::<Address>() {
        Ok(address) => {
            let mut result = ConversionResult {
                data: address,
                metadata,
                is_valid: true,
                validation_errors: Vec::new(),
            };

            // Validar que no sea zero address
            if address == Address::zero() {
                result
                    .validation_errors
                    .push("Address is zero address".to_string());
                result.is_valid = false;
            }

            debug!(result = ?address, valid = result.is_valid, "Address parsing complete");
            Ok(result)
        }
        Err(e) => {
            error!(error = ?e, "Failed to parse address");
            let mut metadata = metadata;
            metadata
                .parsing_errors
                .push(format!("Address parsing error: {}", e));

            Ok(ConversionResult {
                data: Address::zero(),
                metadata,
                is_valid: false,
                validation_errors: vec![format!("Address parsing failed: {}", e)],
            })
        }
    }
}

/// 🔢 CONVERSIÓN U256 A F64 CON DECIMALES
#[instrument(skip(value, decimals), fields(value = ?value, decimals = decimals))]
pub fn u256_to_f64_with_decimals(value: U256, decimals: u8) -> Result<f64> {
    debug!("Converting U256 to f64 with decimals");

    // Convertir a string primero para evitar overflow
    let value_str = value.to_string();
    let divisor = 10u128.pow(decimals as u32);

    // Usar f64 para la conversión
    let value_f64 = value_str
        .parse::<f64>()
        .context("Failed to convert U256 string to f64")?;

    let result = value_f64 / divisor as f64;

    // Validar que no sea NaN o infinito
    if !result.is_finite() {
        return Err(anyhow!(
            "Conversion resulted in non-finite value: {}",
            result
        ));
    }

    debug!(result = result, "U256 to f64 conversion complete");
    Ok(result)
}

/// 📈 STRUCT PARA RESERVAS V2 NORMALIZADAS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedV2Reserves {
    pub reserve0: U256,
    pub reserve1: U256,
    pub reserve0_f64: f64,
    pub reserve1_f64: f64,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub metadata: DataMetadata,
}

impl DataConvertible for NormalizedV2Reserves {
    type RawInput = (String, String, u8, u8, DataSource); // (reserve0_hex, reserve1_hex, dec0, dec1, source)

    #[instrument(skip(raw_input))]
    fn from_raw(raw_input: Self::RawInput, source: DataSource) -> Result<ConversionResult<Self>> {
        let (reserve0_hex, reserve1_hex, dec0, dec1, _source) = raw_input;

        debug!("Converting V2 reserves from raw data");

        let mut parsing_errors = Vec::new();
        let mut validation_errors = Vec::new();

        // Parse reserve0
        let reserve0_result = parse_hex_to_u256(&reserve0_hex)?;
        if !reserve0_result.is_valid {
            parsing_errors.extend(reserve0_result.metadata.parsing_errors);
            validation_errors.extend(reserve0_result.validation_errors);
        }

        // Parse reserve1
        let reserve1_result = parse_hex_to_u256(&reserve1_hex)?;
        if !reserve1_result.is_valid {
            parsing_errors.extend(reserve1_result.metadata.parsing_errors);
            validation_errors.extend(reserve1_result.validation_errors);
        }

        // Convertir a f64 con decimales
        let reserve0_f64 = match u256_to_f64_with_decimals(reserve0_result.data, dec0) {
            Ok(v) => v,
            Err(e) => {
                validation_errors.push(format!("Reserve0 decimal conversion failed: {}", e));
                0.0
            }
        };

        let reserve1_f64 = match u256_to_f64_with_decimals(reserve1_result.data, dec1) {
            Ok(v) => v,
            Err(e) => {
                validation_errors.push(format!("Reserve1 decimal conversion failed: {}", e));
                0.0
            }
        };

        let parsing_errors_clone = parsing_errors.clone();
        let metadata = DataMetadata {
            source,
            timestamp_ms: current_timestamp_ms(),
            block_number: None,
            raw_input: Some(format!(
                "reserve0={}, reserve1={}",
                reserve0_hex, reserve1_hex
            )),
            parsing_errors,
        };

        let data = NormalizedV2Reserves {
            reserve0: reserve0_result.data,
            reserve1: reserve1_result.data,
            reserve0_f64,
            reserve1_f64,
            token0_decimals: dec0,
            token1_decimals: dec1,
            metadata: metadata.clone(),
        };

        let is_valid = validation_errors.is_empty() && parsing_errors_clone.is_empty();

        Ok(ConversionResult {
            data,
            metadata,
            is_valid,
            validation_errors,
        })
    }

    fn validate_internal_consistency(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Validar que las reservas no sean cero (pool vacío)
        if self.reserve0.is_zero() && self.reserve1.is_zero() {
            errors.push("Both reserves are zero - empty pool".to_string());
        }

        // Validar que los valores f64 sean finitos
        if !self.reserve0_f64.is_finite() {
            errors.push("reserve0_f64 is not finite".to_string());
        }
        if !self.reserve1_f64.is_finite() {
            errors.push("reserve1_f64 is not finite".to_string());
        }

        errors
    }
}

/// 📊 STRUCT PARA SLOT0 V3 NORMALIZADO
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedV3Slot0 {
    pub sqrt_price_x96: U256,
    pub tick: i64,
    pub liquidity: u128,
    pub metadata: DataMetadata,
}

impl DataConvertible for NormalizedV3Slot0 {
    type RawInput = (String, String, String, DataSource); // (sqrt_price_hex, tick_hex, liquidity_hex, source)

    #[instrument(skip(raw_input))]
    fn from_raw(raw_input: Self::RawInput, source: DataSource) -> Result<ConversionResult<Self>> {
        let (sqrt_price_hex, tick_hex, liquidity_hex, _source) = raw_input;

        debug!("Converting V3 slot0 from raw data");

        let mut parsing_errors = Vec::new();
        let mut validation_errors = Vec::new();

        // Parse sqrt_price
        let sqrt_price_result = parse_hex_to_u256(&sqrt_price_hex)?;
        if !sqrt_price_result.is_valid {
            parsing_errors.extend(sqrt_price_result.metadata.parsing_errors);
            validation_errors.extend(sqrt_price_result.validation_errors);
        }

        // Parse tick (como i64)
        let tick = match tick_hex.parse::<i64>() {
            Ok(t) => {
                // Validar rango de tick V3
                if t < -887272 || t > 887272 {
                    validation_errors
                        .push(format!("Tick {} out of valid range [-887272, 887272]", t));
                }
                t
            }
            Err(e) => {
                parsing_errors.push(format!("Tick parsing error: {}", e));
                0
            }
        };

        // Parse liquidity
        let liquidity_result = parse_hex_to_u256(&liquidity_hex)?;
        let liquidity = match liquidity_result.data.try_into() {
            Ok(l) => l,
            Err(_) => {
                validation_errors.push("Liquidity value too large for u128".to_string());
                0
            }
        };

        if !liquidity_result.is_valid {
            parsing_errors.extend(liquidity_result.metadata.parsing_errors);
            validation_errors.extend(liquidity_result.validation_errors);
        }

        let parsing_errors_clone = parsing_errors.clone();
        let metadata = DataMetadata {
            source,
            timestamp_ms: current_timestamp_ms(),
            block_number: None,
            raw_input: Some(format!(
                "sqrt_price={}, tick={}, liquidity={}",
                sqrt_price_hex, tick_hex, liquidity_hex
            )),
            parsing_errors,
        };

        let data = NormalizedV3Slot0 {
            sqrt_price_x96: sqrt_price_result.data,
            tick,
            liquidity,
            metadata: metadata.clone(),
        };

        let is_valid = validation_errors.is_empty() && parsing_errors_clone.is_empty();

        Ok(ConversionResult {
            data,
            metadata,
            is_valid,
            validation_errors,
        })
    }

    fn validate_internal_consistency(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Validar que sqrt_price no sea cero
        if self.sqrt_price_x96.is_zero() {
            errors.push("sqrt_price_x96 is zero - invalid pool state".to_string());
        }

        // Validar que liquidity no sea cero
        if self.liquidity == 0 {
            errors.push("Liquidity is zero - invalid pool state".to_string());
        }

        // Validar rango de tick nuevamente
        if self.tick < -887272 || self.tick > 887272 {
            errors.push(format!("Tick {} out of valid range", self.tick));
        }

        errors
    }
}

/// 🏭 DATA PIPELINE PRINCIPAL
#[derive(Debug)]
pub struct DataPipeline {
    audit_mode: bool,
    audit_log: Vec<serde_json::Value>,
}

impl DataPipeline {
    pub fn new(audit_mode: bool) -> Self {
        Self {
            audit_mode,
            audit_log: Vec::new(),
        }
    }

    /// 🔄 Procesa datos crudos a través del pipeline completo
    #[instrument(skip(raw_data, source))]
    pub fn process<T: DataConvertible + Serialize>(
        &mut self,
        raw_data: T::RawInput,
        source: DataSource,
    ) -> Result<ConversionResult<T>> {
        debug!("Processing data through pipeline");

        // 1. Parsing y normalización
        let conversion_result = T::from_raw(raw_data, source)?;

        // 2. Validación interna
        let internal_errors = conversion_result.data.validate_internal_consistency();
        let mut all_validation_errors = conversion_result.validation_errors.clone();
        all_validation_errors.extend(internal_errors);

        // 3. Actualizar validez final
        let is_valid = all_validation_errors.is_empty() && conversion_result.is_valid;

        let final_result = ConversionResult {
            data: conversion_result.data,
            metadata: conversion_result.metadata,
            is_valid,
            validation_errors: all_validation_errors,
        };

        // 4. Logging estructurado
        if self.audit_mode {
            self.log_audit_entry(&final_result)?;
        }

        // 5. Logs de tracing
        if !final_result.is_valid {
            warn!(
                source = %final_result.metadata.source,
                errors = ?final_result.validation_errors,
                "Data validation failed"
            );
        } else {
            debug!(
                source = %final_result.metadata.source,
                "Data processing successful"
            );
        }

        Ok(final_result)
    }

    /// 📝 Registra entrada en audit log
    fn log_audit_entry<T: Serialize>(&mut self, result: &ConversionResult<T>) -> Result<()> {
        let audit_entry = serde_json::json!({
            "timestamp_ms": result.metadata.timestamp_ms,
            "source": result.metadata.source,
            "is_valid": result.is_valid,
            "parsing_errors": result.metadata.parsing_errors,
            "validation_errors": result.validation_errors,
            "raw_input": result.metadata.raw_input,
            "data": result.data,
        });

        self.audit_log.push(audit_entry);
        Ok(())
    }

    /// 💾 Exporta audit log a archivo
    pub fn export_audit_log(&mut self, path: &str) -> Result<()> {
        let json_data = serde_json::to_string_pretty(&self.audit_log)?;
        std::fs::write(path, json_data)?;
        info!("Audit log exported to {}", path);
        Ok(())
    }

    /// 📊 Genera reporte de calidad de datos
    pub fn generate_quality_report(&self) -> HashMap<String, serde_json::Value> {
        let total_entries = self.audit_log.len();
        let valid_entries = self
            .audit_log
            .iter()
            .filter(|entry| entry["is_valid"].as_bool().unwrap_or(false))
            .count();

        let quality_score = if total_entries > 0 {
            valid_entries as f64 / total_entries as f64
        } else {
            1.0
        };

        let mut source_stats = HashMap::new();
        for entry in &self.audit_log {
            let source = entry["source"].as_str().unwrap_or("unknown");
            let is_valid = entry["is_valid"].as_bool().unwrap_or(false);

            let stats = source_stats.entry(source.to_string()).or_insert_with(|| {
                serde_json::json!({
                    "total": 0,
                    "valid": 0,
                    "quality_score": 0.0
                })
            });

            stats["total"] = serde_json::json!(stats["total"].as_i64().unwrap_or(0) + 1);
            if is_valid {
                stats["valid"] = serde_json::json!(stats["valid"].as_i64().unwrap_or(0) + 1);
            }
            let total = stats["total"].as_i64().unwrap_or(0);
            let valid = stats["valid"].as_i64().unwrap_or(0);
            if total > 0 {
                stats["quality_score"] = serde_json::json!(valid as f64 / total as f64);
            }
        }

        HashMap::from([
            (
                "total_entries".to_string(),
                serde_json::json!(total_entries),
            ),
            (
                "valid_entries".to_string(),
                serde_json::json!(valid_entries),
            ),
            (
                "overall_quality_score".to_string(),
                serde_json::json!(quality_score),
            ),
            ("source_stats".to_string(), serde_json::json!(source_stats)),
        ])
    }
}

/// ⏰ TIMESTAMP HELPER
fn current_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_to_u256_valid() {
        let result = parse_hex_to_u256("0x0f4240").unwrap();
        assert!(result.is_valid);
        assert_eq!(result.data, U256::from(1000000u64));
        assert!(result.validation_errors.is_empty());
    }

    #[test]
    fn test_parse_hex_to_u256_zero() {
        let result = parse_hex_to_u256("0x0").unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.data, U256::zero());
        assert!(result
            .validation_errors
            .contains(&"U256 value is zero".to_string()));
    }

    #[test]
    fn test_parse_hex_to_u256_invalid() {
        let result = parse_hex_to_u256("invalid").unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.data, U256::zero());
        assert!(!result.validation_errors.is_empty());
    }

    #[test]
    fn test_parse_address_valid() {
        let addr_str = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1";
        let result = parse_address_to_h160(addr_str).unwrap();
        assert!(result.is_valid);
        assert!(result.validation_errors.is_empty());
    }

    #[test]
    fn test_parse_address_zero() {
        let result = parse_address_to_h160("0x0000000000000000000000000000000000000000").unwrap();
        assert!(!result.is_valid);
        assert!(result
            .validation_errors
            .contains(&"Address is zero address".to_string()));
    }

    #[test]
    fn test_u256_to_f64_with_decimals() {
        let value = U256::from(1000000000000000000u128); // 1 ETH in wei
        let result = u256_to_f64_with_decimals(value, 18).unwrap();
        assert_eq!(result, 1.0);
    }

    #[test]
    fn test_v2_reserves_conversion() {
        let raw_input = (
            "0x0de0b6b3a7640000".to_string(), // 1e18
            "0x0de0b6b3a7640000".to_string(), // 1e18
            18u8,
            18u8,
            DataSource::MulticalV1,
        );

        let result = NormalizedV2Reserves::from_raw(raw_input, DataSource::MulticalV1).unwrap();
        assert!(result.is_valid);
        assert_eq!(result.data.reserve0_f64, 1.0);
        assert_eq!(result.data.reserve1_f64, 1.0);
    }

    #[test]
    fn test_data_pipeline_audit_mode() {
        let mut pipeline = DataPipeline::new(true);

        let raw_input = (
            "0x0de0b6b3a7640000".to_string(),
            "0x0de0b6b3a7640000".to_string(),
            18u8,
            18u8,
            DataSource::MulticalV1,
        );

        let result = pipeline
            .process::<NormalizedV2Reserves>(raw_input, DataSource::MulticalV1)
            .unwrap();
        assert!(result.is_valid);
        assert_eq!(pipeline.audit_log.len(), 1);

        // Test quality report
        let report = pipeline.generate_quality_report();
        assert_eq!(report["total_entries"], 1);
        assert_eq!(report["valid_entries"], 1);
        assert_eq!(report["overall_quality_score"], 1.0);
    }
}
