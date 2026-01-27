// Pool Priority Classifier - Clasifica pools por prioridad de validación
// Determina si un pool debe validarse inmediatamente o puede diferirse

use ethers::types::Address;
use std::collections::HashSet;
use tracing::debug;

use crate::pool_event_extractor::PoolCandidate;

/// Prioridad de validación de un pool
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationPriority {
    /// Pool crítico: tokens blue-chip conocidos, validar SIEMPRE en mismo bloque
    Critical = 4,
    /// Pool alta prioridad: liquidez alta o DEX conocido, validar en mismo bloque si cabe
    High = 3,
    /// Pool prioridad media: tokens conocidos, validar en 1-5 bloques
    Medium = 2,
    /// Pool baja prioridad: desconocido, validar en 5-10 bloques
    Low = 1,
}

impl ValidationPriority {
    /// Convierte un valor numérico a ValidationPriority
    pub fn from_u8(value: u8) -> Self {
        match value {
            4 => ValidationPriority::Critical,
            3 => ValidationPriority::High,
            2 => ValidationPriority::Medium,
            _ => ValidationPriority::Low,
        }
    }

    /// Convierte ValidationPriority a u8 para comparaciones
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Clasificador de prioridad de pools
pub struct PoolPriorityClassifier {
    blue_chip_tokens: HashSet<Address>,
    known_dexes: HashSet<String>,
}

impl PoolPriorityClassifier {
    /// Crea un nuevo clasificador con tokens blue-chip conocidos
    pub fn new(blue_chip_tokens: HashSet<Address>) -> Self {
        let known_dexes: HashSet<String> = [
            "UniswapV2".to_string(),
            "UniswapV3".to_string(),
            "SushiSwapV2".to_string(),
            "CamelotV2".to_string(),
            "CamelotV3".to_string(),
            "PancakeSwapV2".to_string(),
            "TraderJoeV2".to_string(),
            "KyberSwapV3".to_string(),
        ]
        .into_iter()
        .collect();

        Self {
            blue_chip_tokens,
            known_dexes,
        }
    }

    /// Clasifica un pool candidato según su prioridad
    pub fn classify_pool(
        &self,
        candidate: &PoolCandidate,
        known_tokens: &HashSet<Address>,
    ) -> ValidationPriority {
        // Critical: Pool con ambos tokens blue-chip
        let has_blue_chip_token0 = self.blue_chip_tokens.contains(&candidate.token0);
        let has_blue_chip_token1 = self.blue_chip_tokens.contains(&candidate.token1);

        if has_blue_chip_token0 && has_blue_chip_token1 {
            debug!(
                "🔴 [PriorityClassifier] Pool {} classified as CRITICAL (both tokens blue-chip)",
                candidate.address
            );
            return ValidationPriority::Critical;
        }

        // High: Pool con al menos un token blue-chip O DEX conocido
        if has_blue_chip_token0 || has_blue_chip_token1 {
            debug!(
                "🟠 [PriorityClassifier] Pool {} classified as HIGH (has blue-chip token)",
                candidate.address
            );
            return ValidationPriority::High;
        }

        if self.known_dexes.contains(&candidate.dex) {
            debug!(
                "🟠 [PriorityClassifier] Pool {} classified as HIGH (known DEX: {})",
                candidate.address, candidate.dex
            );
            return ValidationPriority::High;
        }

        // Medium: Pool con tokens conocidos (pero no blue-chip)
        let has_known_token0 = known_tokens.contains(&candidate.token0);
        let has_known_token1 = known_tokens.contains(&candidate.token1);

        if has_known_token0 || has_known_token1 {
            debug!(
                "🟡 [PriorityClassifier] Pool {} classified as MEDIUM (has known token)",
                candidate.address
            );
            return ValidationPriority::Medium;
        }

        // Low: Pool completamente desconocido
        debug!(
            "⚪ [PriorityClassifier] Pool {} classified as LOW (unknown tokens/DEX)",
            candidate.address
        );
        ValidationPriority::Low
    }

    /// Clasifica múltiples pools y los separa por prioridad
    pub fn classify_pools(
        &self,
        candidates: &[PoolCandidate],
        known_tokens: &HashSet<Address>,
    ) -> (
        Vec<PoolCandidate>,
        Vec<PoolCandidate>,
        Vec<PoolCandidate>,
        Vec<PoolCandidate>,
    ) {
        let mut critical = Vec::new();
        let mut high = Vec::new();
        let mut medium = Vec::new();
        let mut low = Vec::new();

        for candidate in candidates {
            match self.classify_pool(candidate, known_tokens) {
                ValidationPriority::Critical => critical.push(candidate.clone()),
                ValidationPriority::High => high.push(candidate.clone()),
                ValidationPriority::Medium => medium.push(candidate.clone()),
                ValidationPriority::Low => low.push(candidate.clone()),
            }
        }

        (critical, high, medium, low)
    }
}

impl Default for PoolPriorityClassifier {
    fn default() -> Self {
        // Tokens blue-chip por defecto (WETH, USDC, USDT en Arbitrum)
        let blue_chip_tokens: HashSet<Address> = [
            "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", // WETH
            "0xFF970A61A04b1cA14834A43f5dE4533eBDDB5CC8", // USDC
            "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", // USDT
        ]
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();

        Self::new(blue_chip_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_classify_blue_chip_pool() {
        let classifier = PoolPriorityClassifier::default();
        let known_tokens = HashSet::new();

        let candidate = PoolCandidate {
            address: Address::from_str("0x1111111111111111111111111111111111111111").unwrap(),
            dex: "UniswapV3".to_string(),
            factory: Address::zero(),
            token0: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
            token1: Address::from_str("0xFF970A61A04b1cA14834A43f5dE4533eBDDB5CC8").unwrap(), // USDC
            fee_bps: Some(500),
            discovered_at_block: 100,
        };

        let priority = classifier.classify_pool(&candidate, &known_tokens);
        assert_eq!(priority, ValidationPriority::Critical);
    }

    #[test]
    fn test_classify_unknown_pool() {
        let classifier = PoolPriorityClassifier::default();
        let known_tokens = HashSet::new();

        let candidate = PoolCandidate {
            address: Address::from_str("0x2222222222222222222222222222222222222222").unwrap(),
            dex: "UnknownDEX".to_string(),
            factory: Address::zero(),
            token0: Address::from_str("0x3333333333333333333333333333333333333333").unwrap(),
            token1: Address::from_str("0x4444444444444444444444444444444444444444").unwrap(),
            fee_bps: Some(300),
            discovered_at_block: 100,
        };

        let priority = classifier.classify_pool(&candidate, &known_tokens);
        assert_eq!(priority, ValidationPriority::Low);
    }
}
