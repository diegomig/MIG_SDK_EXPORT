// src/pools.rs

use ethers::prelude::{Address, U256, Provider, Http};
use ethers::middleware::Middleware;
use ethers::types::{Bytes, TransactionRequest};
use ethers::utils::keccak256;
use tokio::time::{timeout, Duration};
use tracing::warn;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapKind {
    ExactInput,
    ExactOutput,
}
use serde::{Serialize, Deserialize};
use anyhow::Result;
use std::sync::Arc;
use std::convert::TryInto;


/// Unified pool representation across all DEX protocols.
///
/// This enum provides a type-safe way to represent pools from different DEX protocols
/// while maintaining protocol-specific data structures.
///
/// ## Supported Protocols
///
/// - **Uniswap V2**: Constant product formula pools
/// - **Uniswap V3**: Concentrated liquidity pools
/// - **Balancer**: Weighted pools with multiple tokens
/// - **Curve**: StableSwap pools for stablecoins
#[derive(Debug, Clone)]
pub enum Pool {
    /// Uniswap V2-style pool (constant product)
    UniswapV2(UniswapV2Pool),
    /// Uniswap V3-style pool (concentrated liquidity)
    UniswapV3(UniswapV3Pool),
    /// Balancer weighted pool
    BalancerWeighted(BalancerWeightedPool),
    /// Curve StableSwap pool
    CurveStableSwap(CurveStableSwapPool),
}

/// Uniswap V2-style pool with constant product formula.
///
/// Uses the x * y = k formula where reserves are maintained in a constant product relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniswapV2Pool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: u128,
    pub reserve1: u128,
    pub dex: &'static str,
}

impl UniswapV2Pool {
    pub async fn fetch_state(mut self, provider: Arc<Provider<Http>>) -> Result<Self> {
        // getReserves() selector 0x0902f1ac
        let selector = &keccak256("getReserves()")[..4];
        let mut data = vec![0u8; 4];
        data.copy_from_slice(selector);

        let call = TransactionRequest::new()
            .to(self.address)
            .data(Bytes::from(data));

        if let Ok(Ok(bytes)) = timeout(Duration::from_secs(8), provider.call(&call.into(), None)).await {
            if bytes.0.len() >= 96 {
                let r0 = U256::from_big_endian(&bytes.0[0..32]);
                let r1 = U256::from_big_endian(&bytes.0[32..64]);
                // Use TryInto to avoid panics on overflow
                self.reserve0 = r0.try_into().unwrap_or(u128::MAX);
                self.reserve1 = r1.try_into().unwrap_or(u128::MAX);
            }
        } else {
            warn!("V2 getReserves timeout/err for {:?}", self.address);
        }
        Ok(self)
    }

    /// Calculates the price of token0 in terms of token1, scaled by 1e18.
    pub fn price(&self, token0_decimals: u8, token1_decimals: u8) -> U256 {
        if self.reserve0 == 0 || self.reserve1 == 0 {
            return U256::zero();
        }
        let r0 = U256::from(self.reserve0);
        let r1 = U256::from(self.reserve1);
        let scale_factor = U256::exp10(18 + token0_decimals as usize - token1_decimals as usize);
        (r1 * scale_factor) / r0
    }
}

/// Uniswap V3 pool with concentrated liquidity.
///
/// V3 pools use a tick-based pricing system where liquidity is concentrated in specific
/// price ranges, enabling more capital efficiency than V2 pools.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UniswapV3Pool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub tick: i32,
    pub dex: &'static str,
}

use ethers::core::types::{U512};

impl UniswapV3Pool {
    /// Creates a new UniswapV3Pool instance.
    pub fn new(
        address: Address,
        token0: Address,
        token1: Address,
        fee: u32,
        sqrt_price_x96: U256,
        liquidity: u128,
        tick: i32,
        dex: &'static str,
    ) -> Self {
        Self {
            address,
            token0,
            token1,
            fee,
            sqrt_price_x96,
            liquidity,
            tick,
            dex,
        }
    }

    pub async fn fetch_state(mut self, provider: Arc<Provider<Http>>) -> Result<Self> {
        // slot0() selector 0x3850c7bd, liquidity() selector 0x1a686502
        // slot0 returns (uint160 sqrtPriceX96, int24 tick, ...)
        let mut slot0_selector = vec![0u8; 4];
        slot0_selector.copy_from_slice(&keccak256("slot0()")[..4]);
        let slot0_call = TransactionRequest::new()
            .to(self.address)
            .data(Bytes::from(slot0_selector));

        if let Ok(Ok(bytes)) = timeout(Duration::from_secs(8), provider.call(&slot0_call.into(), None)).await {
            // first 32 bytes contain sqrtPriceX96 in the lower 160 bits
            if bytes.0.len() >= 64 {
                let sqrt = U256::from_big_endian(&bytes.0[0..32]);
                // tick encoded as int24 in next 32 bytes (sign-extended)
                let mut tick_raw = [0u8; 32];
                tick_raw.copy_from_slice(&bytes.0[32..64]);
                // interpret last 3 bytes as signed int24
                let t_bytes = &tick_raw[29..32];
                let t_u32 = ((t_bytes[0] as u32) << 16) | ((t_bytes[1] as u32) << 8) | (t_bytes[2] as u32);
                let tick_i32 = if (t_u32 & 0x800000) != 0 { (t_u32 as i32) | !0xFFFFFF } else { t_u32 as i32 };
                self.sqrt_price_x96 = sqrt;
                self.tick = tick_i32;
            }
        } else {
            warn!("V3 slot0 timeout/err for {:?}", self.address);
        }

        let mut liq_selector = vec![0u8; 4];
        liq_selector.copy_from_slice(&keccak256("liquidity()")[..4]);
        let liq_call = TransactionRequest::new()
            .to(self.address)
            .data(Bytes::from(liq_selector));
        if let Ok(Ok(bytes)) = timeout(Duration::from_secs(8), provider.call(&liq_call.into(), None)).await {
            if bytes.0.len() >= 32 {
                let liq = U256::from_big_endian(&bytes.0[0..32]);
                self.liquidity = liq.as_u128();
            }
        } else {
            warn!("V3 liquidity() timeout/err for {:?}", self.address);
        }
        Ok(self)
    }

    /// Calculates the price of token0 in terms of token1, scaled by 1e18.
    pub fn price(&self, token0_decimals: u8, token1_decimals: u8) -> U256 {
        if self.sqrt_price_x96.is_zero() {
            return U256::zero();
        }
        // price = (sqrt_price_x96^2 * 10^d0) / (2^192 * 10^d1)
        // To get price scaled by 1e18, we do:
        // price = (sqrt_price_x96^2 * 10^d0 * 10^18) / (2^192 * 10^d1)
        // which is equivalent to:
        // price = (sqrt_price_x96^2 * 10^(18 + d0 - d1)) / 2^192

        let price_x96_sq: U512 = self.sqrt_price_x96.full_mul(self.sqrt_price_x96);

        let scale_diff = 18 + token0_decimals as i32 - token1_decimals as i32;

        let scaled_price = if scale_diff >= 0 {
            let scale_factor = U256::exp10(scale_diff as usize);
            price_x96_sq * U512::from(scale_factor)
        } else {
            let scale_factor = U256::exp10(-scale_diff as usize);
            price_x96_sq / U512::from(scale_factor)
        };

        let price_u512 = scaled_price >> 192;

        price_u512.try_into().unwrap_or(U256::max_value())
    }
}

/// Represents a Balancer weighted pool.
#[derive(Debug, Clone)]
pub struct BalancerWeightedPool {
    pub address: Address,
    pub pool_id: [u8; 32],
    pub tokens: Vec<Address>,
    pub balances: Vec<U256>,
    pub weights: Vec<U256>,
    pub swap_fee: U256,
    pub dex: &'static str,
}

impl BalancerWeightedPool {
    pub async fn fetch_state(self, _provider: Arc<Provider<Http>>) -> Result<Self> {
        // TODO: Implement real state fetching logic
        Ok(self)
    }
}

/// Represents a Curve stableswap pool.
#[derive(Debug, Clone)]
pub struct CurveStableSwapPool {
    pub address: Address,
    pub tokens: Vec<Address>,
    pub balances: Vec<U256>,
    pub a: U256,
    pub fee: U256,
    pub dex: &'static str,
}

impl CurveStableSwapPool {
    pub async fn fetch_state(self, _provider: Arc<Provider<Http>>) -> Result<Self> {
        // TODO: Implement real state fetching logic
        Ok(self)
    }
}

// Implementation block for Pool to easily access common data.
impl Pool {
    pub async fn fetch_state(self, provider: Arc<Provider<Http>>) -> Result<Self> {
        match self {
            Pool::UniswapV2(p) => Ok(Pool::UniswapV2(p.fetch_state(provider).await?)),
            Pool::UniswapV3(p) => Ok(Pool::UniswapV3(p.fetch_state(provider).await?)),
            Pool::BalancerWeighted(p) => Ok(Pool::BalancerWeighted(p.fetch_state(provider).await?)),
            Pool::CurveStableSwap(p) => Ok(Pool::CurveStableSwap(p.fetch_state(provider).await?)),
        }
    }

    pub fn address(&self) -> Address {
        match self {
            Pool::UniswapV2(p) => p.address,
            Pool::UniswapV3(p) => p.address,
            Pool::BalancerWeighted(p) => p.address,
            Pool::CurveStableSwap(p) => p.address,
        }
    }

    pub fn tokens(&self) -> Vec<Address> {
        match self {
            Pool::UniswapV2(p) => vec![p.token0, p.token1],
            Pool::UniswapV3(p) => vec![p.token0, p.token1],
            Pool::BalancerWeighted(p) => p.tokens.clone(),
            Pool::CurveStableSwap(p) => p.tokens.clone(),
        }
    }

    pub fn dex(&self) -> &'static str {
        match self {
            Pool::UniswapV2(p) => p.dex,
            Pool::UniswapV3(p) => p.dex,
            Pool::BalancerWeighted(p) => p.dex,
            Pool::CurveStableSwap(p) => p.dex,
        }
    }

    pub fn kind(&self) -> SwapKind {
        // NOTE: Simplified SwapKind for topology SDK (no trading-specific variants)
        match self {
            Pool::UniswapV2(_) => SwapKind::ExactInput, // Generic swap kind
            Pool::UniswapV3(_) => SwapKind::ExactInput,
            Pool::BalancerWeighted(_) => SwapKind::ExactInput,
            Pool::CurveStableSwap(_) => SwapKind::ExactInput,
        }
    }

    pub fn token0(&self) -> Address {
        match self {
            Pool::UniswapV2(p) => p.token0,
            Pool::UniswapV3(p) => p.token0,
            _ => Address::zero(),
        }
    }

    pub fn token1(&self) -> Address {
        match self {
            Pool::UniswapV2(p) => p.token1,
            Pool::UniswapV3(p) => p.token1,
            _ => Address::zero(),
        }
    }

    pub fn reserve0(&self) -> U256 {
        match self {
            Pool::UniswapV2(p) => p.reserve0.into(),
            _ => U256::zero(),
        }
    }

    pub fn reserve1(&self) -> U256 {
        match self {
            Pool::UniswapV2(p) => p.reserve1.into(),
            _ => U256::zero(),
        }
    }

    pub fn fee_bps(&self) -> u32 {
        match self {
            Pool::UniswapV2(_) => 30,
            Pool::UniswapV3(p) => p.fee,
            _ => 0,
        }
    }
}
