//! # Router Module
//!
//! Provides routing primitives for liquidity path discovery across DEX protocols.
//! This module defines the core data structures for representing swap steps and routes.

use ethers::prelude::Address;
use ethers::types::U256;
use serde::{Deserialize, Serialize};

/// DEX protocol identifier for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DexId {
    #[default]
    UniswapV2,
    SushiSwapV2,
    UniswapV3,
    CamelotV2,
    CamelotV3,
    PancakeSwap,
    KyberSwap,
    TraderJoe,
    Balancer,
    Curve,
}

impl std::fmt::Display for DexId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DexId::UniswapV2 => write!(f, "UniswapV2"),
            DexId::SushiSwapV2 => write!(f, "SushiSwapV2"),
            DexId::UniswapV3 => write!(f, "UniswapV3"),
            DexId::CamelotV2 => write!(f, "CamelotV2"),
            DexId::CamelotV3 => write!(f, "CamelotV3"),
            DexId::PancakeSwap => write!(f, "PancakeSwap"),
            DexId::KyberSwap => write!(f, "KyberSwap"),
            DexId::TraderJoe => write!(f, "TraderJoe"),
            DexId::Balancer => write!(f, "Balancer"),
            DexId::Curve => write!(f, "Curve"),
        }
    }
}

/// Swap type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SwapKind {
    #[default]
    V2,
    V3,
    Balancer,
    Curve,
}

/// A single swap step in a liquidity route.
///
/// Represents one hop in a multi-hop path, containing all information needed
/// to execute or simulate the swap.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SwapStep {
    /// DEX protocol identifier
    pub dex: DexId,
    /// Pool contract address
    pub pool: Address,
    /// Input token address
    pub token_in: Address,
    /// Output token address
    pub token_out: Address,
    /// Fee in basis points (e.g., 30 = 0.3%)
    pub fee_bps: u32,
    /// Swap type (V2, V3, Balancer, Curve)
    pub kind: SwapKind,
    /// Pool weight for routing prioritization
    pub weight: f64,
    /// Input reserve (for V2-like pools)
    pub reserve_in: U256,
    /// Output reserve (for V2-like pools)
    pub reserve_out: U256,
    /// Pool ID (for Balancer pools)
    pub pool_id: Option<[u8; 32]>,
    /// Token indices (for Curve pools)
    pub token_indices: Option<(i128, i128)>,
}

/// A candidate route through multiple pools.
///
/// Represents a complete path from entry token back to entry token,
/// typically used for circular routing scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRoute {
    /// Sequence of swap steps forming the route
    pub steps: Vec<SwapStep>,
    /// Entry token address (where the route starts and ends)
    pub entry_token: Address,
}

impl CandidateRoute {
    /// Generate a unique route identifier based on pool addresses.
    pub fn get_id(&self) -> String {
        self.steps
            .iter()
            .map(|s| format!("{:?}", s.pool))
            .collect::<Vec<_>>()
            .join("-")
    }
}

/// Map DEX name string to DexId enum.
pub fn map_dex(name: &str) -> DexId {
    match name {
        "UniswapV2" | "CamelotV2" | "PancakeSwap" | "TraderJoe" => DexId::UniswapV2,
        "SushiSwapV2" => DexId::SushiSwapV2,
        "UniswapV3" | "CamelotV3" | "KyberSwap" => DexId::UniswapV3,
        "Balancer" => DexId::Balancer,
        "Curve" => DexId::Curve,
        _ => DexId::UniswapV2,
    }
}

/// Map DEX name string to SwapKind enum.
pub fn map_kind(name: &str) -> SwapKind {
    match name {
        "UniswapV2" | "SushiSwapV2" | "CamelotV2" | "PancakeSwap" | "TraderJoe" => SwapKind::V2,
        "UniswapV3" | "CamelotV3" | "KyberSwap" => SwapKind::V3,
        "Balancer" => SwapKind::Balancer,
        "Curve" => SwapKind::Curve,
        _ => SwapKind::V2,
    }
}

/// Extension trait for SwapStep operations.
pub trait SwapStepExt {
    /// Reverse the direction of a swap step.
    fn reverse(&self) -> SwapStep;
}

impl SwapStepExt for SwapStep {
    fn reverse(&self) -> SwapStep {
        let mut rev = self.clone();
        std::mem::swap(&mut rev.token_in, &mut rev.token_out);
        std::mem::swap(&mut rev.reserve_in, &mut rev.reserve_out);
        rev
    }
}
