use anyhow::Result;
use async_trait::async_trait;
use ethers::prelude::*;
use log::{info, warn};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::contracts::{
    i_uniswap_v3_factory::PoolCreatedFilter, uniswap_v3::UniswapV3Pool as ContractUniswapV3Pool,
    IUniswapV3Factory,
};
use crate::dex_adapter::{DexAdapter, PoolMeta};
use crate::multicall::{Call, Multicall};
use crate::pools::{Pool, UniswapV3Pool as PoolUniswapV3Pool};
use crate::rpc_pool::RpcPool;
use crate::utils;

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct KyberSwapAdapter {
    factory_address: Address,
    multicall_address: Address,
    multicall_batch_size: usize,
    rpc_pool: Arc<RpcPool>,
}

impl KyberSwapAdapter {
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
impl DexAdapter for KyberSwapAdapter {
    fn name(&self) -> &'static str {
        "KyberSwap"
    }

    async fn discover_pools(
        &self,
        from_block: u64,
        to_block: u64,
        chunk_size: u64,
        max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>> {
        info!(
            "Discovering new {} pools from block {} to {}",
            self.name(),
            from_block,
            to_block
        );

        let block_chunks = utils::create_block_chunks(from_block, to_block, chunk_size);

        let futures_iter = block_chunks.into_iter().map(|(start, end)| {
            let self_clone = self.clone();
            async move {
                let mut attempts = 0;
                loop {
                    // ✅ FLIGHT RECORDER: Use get_next_provider_with_endpoint to get endpoint for recording
                    let (provider, _permit, endpoint) = self_clone.rpc_pool.get_next_provider_with_endpoint().await?;
                    let factory = IUniswapV3Factory::new(self_clone.factory_address, Arc::clone(&provider));

                    // ✅ FLIGHT RECORDER: Use get_logs_with_recording instead of event.query()
                    use ethers::types::{Filter, H256};
                    use std::str::FromStr;
                    // PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickSpacing, address pool)
                    let pool_created_sig = H256::from_str("0x783cca1c0412dd0d695e7845682c51a44f2b8f17a9b519f3f9be354f23f313bb")
                        .unwrap_or_else(|_| H256::zero());
                    let filter = Filter::new()
                        .address(self_clone.factory_address)
                        .from_block(start)
                        .to_block(end)
                        .topic0(pool_created_sig);

                    info!("Querying {} PoolCreated events from block {} to {}", self_clone.name(), start, end);
                    match self_clone.rpc_pool.get_logs_with_recording(&provider, &filter, &endpoint).await {
                        Ok(logs) => {
                            // Decode logs manually from topics and data
                            let mut decoded_logs = Vec::new();

                            for log in logs {
                                // PoolCreated: topics[0] = signature, topics[1] = token0, topics[2] = token1, topics[3] = fee, data[12:32] = pool
                                if log.topics.len() >= 4 && log.data.len() >= 32 {
                                    let token0 = Address::from_slice(&log.topics[1].as_bytes()[12..]);
                                    let token1 = Address::from_slice(&log.topics[2].as_bytes()[12..]);
                                    let fee = ethers::types::U256::from_big_endian(&log.topics[3].as_bytes()[29..32]);
                                    let pool = Address::from_slice(&log.data.as_ref()[12..32]);

                                    // Create PoolCreatedFilter manually
                                    decoded_logs.push(PoolCreatedFilter {
                                        token_0: token0,
                                        token_1: token1,
                                        fee: fee.as_u32(),
                                        pool,
                                        ..Default::default()
                                    });
                                }
                            }
                            return Ok(decoded_logs);
                        },
                        Err(e) => {
                            let error_string = e.to_string().to_lowercase();
                            if error_string.contains("429") || error_string.contains("too many requests") || error_string.contains("limit exceeded") {
                                self_clone.rpc_pool.report_rate_limit_error(&provider);
                            } else {
                                self_clone.rpc_pool.mark_as_unhealthy(&provider);
                            }

                            attempts += 1;
                            if attempts >= MAX_RETRIES {
                                return Err(anyhow::anyhow!("Failed after {} attempts: {}", attempts, e));
                            }
                            warn!("Pool discovery for {} failed, retrying in {:?}. Attempt {}/{}. Error: {}", self_clone.name(), RETRY_DELAY, attempts, MAX_RETRIES, e);
                            sleep(RETRY_DELAY).await;
                        }
                    }
                }
            }
        });

        let results: Vec<Vec<PoolCreatedFilter>> = futures_util::stream::iter(futures_iter)
            .buffer_unordered(max_concurrency)
            .filter_map(|res| async { res.ok() })
            .collect()
            .await;

        let all_pools: Vec<PoolMeta> = results
            .into_iter()
            .flatten()
            .map(|log| PoolMeta {
                address: log.pool,
                factory: Some(self.factory_address),
                pool_id: None,
                token0: log.token_0,
                token1: log.token_1,
                fee: Some(log.fee),
                dex: self.name(),
                pool_type: None,
            })
            .collect();

        info!("Found {} {} pools in total", all_pools.len(), self.name());
        Ok(all_pools)
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
                .flat_map(|pool| {
                    let pool_contract =
                        ContractUniswapV3Pool::new(pool.address, Arc::clone(&provider));
                    vec![
                        Call {
                            target: pool.address,
                            call_data: pool_contract.slot_0().calldata().unwrap(),
                        },
                        Call {
                            target: pool.address,
                            call_data: pool_contract.liquidity().calldata().unwrap(),
                        },
                    ]
                })
                .collect();

            match multicall.run(calls, None).await {
                Ok(results) => {
                    let dummy_pool =
                        ContractUniswapV3Pool::new(Address::zero(), Arc::clone(&provider));
                    let slot0_fn = dummy_pool.abi().function("slot0")?;
                    let liquidity_fn = dummy_pool.abi().function("liquidity")?;

                    let mut fetched_pools = Vec::new();
                    for (i, pool_meta) in pools.iter().enumerate() {
                        if let (Some(slot0_data), Some(liquidity_data)) =
                            (results.get(i * 2), results.get(i * 2 + 1))
                        {
                            if let (Ok(slot0_decoded), Ok(liquidity_decoded)) = (
                                slot0_fn.decode_output(slot0_data),
                                liquidity_fn.decode_output(liquidity_data),
                            ) {
                                if let (Some(sqrt_price_x96), Some(tick), Some(liquidity)) = (
                                    slot0_decoded[0].clone().into_uint(),
                                    slot0_decoded[1]
                                        .clone()
                                        .into_int()
                                        .and_then(|i| i.try_into().ok()),
                                    liquidity_decoded[0]
                                        .clone()
                                        .into_uint()
                                        .and_then(|u| u.try_into().ok()),
                                ) {
                                    fetched_pools.push(Pool::UniswapV3(PoolUniswapV3Pool {
                                        address: pool_meta.address,
                                        token0: pool_meta.token0,
                                        token1: pool_meta.token1,
                                        fee: pool_meta.fee.unwrap_or(0),
                                        sqrt_price_x96,
                                        liquidity,
                                        tick,
                                        dex: pool_meta.dex,
                                    }));
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
                            "Failed to fetch pool state after {} attempts: {}",
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
