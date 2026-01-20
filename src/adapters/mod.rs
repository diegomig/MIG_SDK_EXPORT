// DEX Adapters Module
// Provides unified interface for interacting with different DEX protocols

pub mod uniswap_v2;
pub mod uniswap_v3;
pub mod balancer_v2;
pub mod balancer_v3;
pub mod curve;
pub mod camelot_v2;
pub mod camelot_v3;
pub mod pancakeswap;
pub mod traderjoe;
pub mod kyberswap;

// Re-export the trait
pub use crate::dex_adapter::DexAdapter;
