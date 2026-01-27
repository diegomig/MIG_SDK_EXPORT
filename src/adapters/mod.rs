// DEX Adapters Module
// Provides unified interface for interacting with different DEX protocols

pub mod balancer_v2;
pub mod balancer_v3;
pub mod camelot_v2;
pub mod camelot_v3;
pub mod curve;
pub mod kyberswap;
pub mod pancakeswap;
pub mod traderjoe;
pub mod uniswap_v2;
pub mod uniswap_v3;

// Re-export the trait
pub use crate::dex_adapter::DexAdapter;
