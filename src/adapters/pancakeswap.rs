use anyhow::Result;
use async_trait::async_trait;
use ethers::prelude::*;
use log::{info, warn};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::contracts::{
    i_uniswap_v2_factory::PairCreatedFilter, IUniswapV2Factory, IUniswapV2Pair,
};
use crate::dex_adapter::{DexAdapter, PoolMeta};
use crate::multicall::{Call, Multicall};
use crate::pools::{Pool, UniswapV2Pool};
use crate::rpc_pool::RpcPool;

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct PancakeSwapAdapter {
    factory_address: Address,
    multicall_address: Address,
    multicall_batch_size: usize,
    rpc_pool: Arc<RpcPool>,
}

impl PancakeSwapAdapter {
    pub fn new(
        factory_address: Address,
        multicall_address: Address,
        multicall_batch_size: usize,
        rpc_pool: Arc<RpcPool>,
    ) -> Self {
        Self {
            factory_address,
            multicall_address,
            multicall_batch_size,
            rpc_pool,
        }
    }
}

#[async_trait]
impl DexAdapter for PancakeSwapAdapter {
    fn name(&self) -> &'static str {
        "PancakeSwap"
    }

    async fn discover_pools(
        &self,
        from_block: u64,
        to_block: u64,
        _chunk_size: u64,
        _max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>> {
        info!(
            "Discovering new V2 pools for factory {} from block {} to {}",
            self.factory_address, from_block, to_block
        );
        let mut attempts = 0;
        loop {
            // ✅ FLIGHT RECORDER: Use get_next_provider_with_endpoint to get endpoint for recording
            let (provider, _permit, endpoint) =
                self.rpc_pool.get_next_provider_with_endpoint().await?;
            let factory = IUniswapV2Factory::new(self.factory_address, Arc::clone(&provider));

            // ✅ FLIGHT RECORDER: Use get_logs_with_recording instead of event_filter.query()
            use ethers::types::{Filter, H256};
            use std::str::FromStr;
            // PairCreated(address indexed token0, address indexed token1, address pair, uint)
            let pair_created_sig = H256::from_str(
                "0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9",
            )
            .unwrap_or_else(|_| H256::zero());
            let filter = Filter::new()
                .address(self.factory_address)
                .from_block(from_block)
                .to_block(to_block)
                .topic0(pair_created_sig);

            match self
                .rpc_pool
                .get_logs_with_recording(&provider, &filter, &endpoint)
                .await
            {
                Ok(logs) => {
                    // Decode logs manually from topics and data
                    let mut discovered_pools = Vec::new();

                    for log in logs {
                        // PairCreated: topics[0] = signature, topics[1] = token0, topics[2] = token1, data[12:32] = pair
                        if log.topics.len() >= 3 && log.data.len() >= 32 {
                            let token0 = Address::from_slice(&log.topics[1].as_bytes()[12..]);
                            let token1 = Address::from_slice(&log.topics[2].as_bytes()[12..]);
                            let pair = Address::from_slice(&log.data.as_ref()[12..32]);

                            discovered_pools.push(PoolMeta {
                                address: pair,
                                factory: Some(self.factory_address),
                                pool_id: None,
                                fee: Some(250), // Standard PancakeSwap fee is 0.25%
                                token0,
                                token1,
                                dex: self.name(),
                                pool_type: Some("UniswapV2".to_string()),
                            });
                        }
                    }

                    info!(
                        "Discovered {} new {} pools.",
                        discovered_pools.len(),
                        self.name()
                    );
                    return Ok(discovered_pools);
                }
                Err(e) => {
                    let error_string = e.to_string().to_lowercase();
                    if error_string.contains("429")
                        || error_string.contains("too many requests")
                        || error_string.contains("limit exceeded")
                    {
                        self.rpc_pool.report_rate_limit_error(&provider);
                    } else {
                        self.rpc_pool.mark_as_unhealthy(&provider);
                    }
                    attempts += 1;
                    if attempts >= MAX_RETRIES {
                        return Err(anyhow::anyhow!(
                            "Failed to discover pools for {} after {} attempts: {}",
                            self.name(),
                            attempts,
                            e
                        ));
                    }
                    warn!(
                        "Discover pools for {} failed, retrying in {:?}. Attempt {}/{}. Error: {}",
                        self.name(),
                        RETRY_DELAY,
                        attempts,
                        MAX_RETRIES,
                        e
                    );
                    sleep(RETRY_DELAY).await;
                }
            }
        }
    }

    async fn fetch_pool_state(&self, pools: &[PoolMeta]) -> Result<Vec<Pool>> {
        let mut attempts = 0;
        loop {
            let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
            let multicall = Multicall::new(
                Arc::clone(&provider),
                self.multicall_address,
                self.multicall_batch_size,
            );
            let calls: Vec<_> = pools
                .iter()
                .map(|pool| {
                    let pair_contract = IUniswapV2Pair::new(pool.address, Arc::clone(&provider));
                    let call_data = pair_contract.get_reserves().calldata().unwrap();
                    Call {
                        target: pool.address,
                        call_data,
                    }
                })
                .collect();

            match multicall.run(calls.clone(), None).await {
                Ok(results) => {
                    let dummy_pair = IUniswapV2Pair::new(Address::zero(), Arc::clone(&provider));
                    let get_reserves_fn = dummy_pair.abi().function("getReserves")?;
                    let mut fetched_pools = Vec::new();
                    for (i, pool_meta) in pools.iter().enumerate() {
                        if let Some(result_data) = results.get(i) {
                            if !result_data.is_empty() {
                                if let Ok(decoded) = get_reserves_fn.decode_output(result_data) {
                                    if let (Some(reserve0), Some(reserve1)) = (
                                        decoded[0]
                                            .clone()
                                            .into_uint()
                                            .and_then(|u| u.try_into().ok()),
                                        decoded[1]
                                            .clone()
                                            .into_uint()
                                            .and_then(|u| u.try_into().ok()),
                                    ) {
                                        fetched_pools.push(Pool::UniswapV2(UniswapV2Pool {
                                            address: pool_meta.address,
                                            token0: pool_meta.token0,
                                            token1: pool_meta.token1,
                                            reserve0,
                                            reserve1,
                                            dex: pool_meta.dex,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                    return Ok(fetched_pools);
                }
                Err(e) => {
                    let error_string = e.to_string().to_lowercase();
                    if error_string.contains("429")
                        || error_string.contains("too many requests")
                        || error_string.contains("limit exceeded")
                    {
                        self.rpc_pool.report_rate_limit_error(&provider);
                    } else {
                        self.rpc_pool.mark_as_unhealthy(&provider);
                    }
                    attempts += 1;
                    if attempts >= MAX_RETRIES {
                        return Err(anyhow::anyhow!(
                            "Failed to fetch pool state for {} after {} attempts: {}",
                            self.name(),
                            attempts,
                            e
                        ));
                    }
                    warn!("Fetch pool state for {} failed, retrying in {:?}. Attempt {}/{}. Error: {}", self.name(), RETRY_DELAY, attempts, MAX_RETRIES, e);
                    sleep(RETRY_DELAY).await;
                }
            }
        }
    }
}
