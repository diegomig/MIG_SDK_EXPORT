use anyhow::Result;
use async_trait::async_trait;
use ethers::prelude::*;
use log::{info, warn};
use serde::Deserialize;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::contracts::{i_balancer_v2_vault::PoolRegisteredFilter, IBalancerV2Vault};
use crate::dex_adapter::{DexAdapter, PoolMeta};
use crate::multicall::{Call, Multicall};
use crate::pools::{BalancerWeightedPool, Pool};
use crate::rpc_pool::RpcPool;
use ethers::prelude::abigen;

abigen!(
    IWeightedPool,
    r#"[
        function getSwapFeePercentage() external view returns (uint256)
        function getNormalizedWeights() external view returns (uint256[] memory)
    ]"#,
);

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

const BALANCER_VAULT_ADDRESS: &str = "0xBA12222222228d8Ba445958a75a0704d566BF2C8";

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct PoolData {
    id: String,
    address: String,
    pool_type: String,
}
#[derive(Deserialize, Debug)]
struct PoolsResponse {
    pools: Vec<PoolData>,
}
#[derive(Deserialize, Debug)]
struct GraphQLResponse {
    data: PoolsResponse,
}

#[derive(Clone)]
pub struct BalancerAdapter {
    rpc_pool: Arc<RpcPool>,
    multicall_address: Address,
    multicall_batch_size: usize,
}

impl BalancerAdapter {
    pub fn new(
        rpc_pool: Arc<RpcPool>,
        multicall_address: Address,
        multicall_batch_size: usize,
    ) -> Self {
        Self {
            rpc_pool,
            multicall_address,
            multicall_batch_size,
        }
    }
}

#[async_trait]
impl DexAdapter for BalancerAdapter {
    fn name(&self) -> &'static str {
        "Balancer"
    }

    async fn discover_pools(
        &self,
        from_block: u64,
        to_block: u64,
        chunk_size: u64,
        _max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>> {
        // 🔧 FIX: Balancer actually DOES use events, so keep normal behavior
        info!(
            "Discovering new Balancer pools from block {} to {}",
            from_block, to_block
        );
        let vault_address: Address = BALANCER_VAULT_ADDRESS.parse()?;

        let block_chunks = crate::utils::create_block_chunks(from_block, to_block, chunk_size);

        let futures_iter = block_chunks.into_iter().map(|(start, end)| {
            let self_clone = self.clone();
            async move {
                let mut attempts = 0;
                loop {
                    let (provider, _permit) = self_clone.rpc_pool.get_next_provider().await?;
                    let vault_contract = IBalancerV2Vault::new(vault_address, Arc::clone(&provider));
                    let event = vault_contract.event::<PoolRegisteredFilter>();

                    info!("Querying Balancer PoolRegistered events from block {} to {}", start, end);
                    match event.from_block(start).to_block(end).query().await {
                        Ok(logs) => return Ok(logs),
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

        // Process Balancer chunks sequentially to avoid rate limits
        let mut results: Vec<Vec<PoolRegisteredFilter>> = Vec::new();
        for future in futures_iter {
            // Add delay between each chunk to avoid rate limits
            tokio::time::sleep(Duration::from_millis(500)).await;

            match future.await {
                Ok(chunk_result) => results.push(chunk_result),
                Err(e) => {
                    warn!("Failed to process Balancer chunk: {}", e);
                    // Continue with next chunk instead of failing completely
                }
            }
        }

        let all_logs: Vec<PoolRegisteredFilter> = results.into_iter().flatten().collect();
        info!("Found {} PoolRegistered events in total", all_logs.len());

        let mut pools_meta: Vec<PoolMeta> = all_logs
            .into_iter()
            .map(|log| PoolMeta {
                address: log.pool_address,
                factory: Some(vault_address),
                pool_id: Some(log.pool_id),
                fee: None,
                token0: Address::zero(),
                token1: Address::zero(),
                dex: self.name(),
                pool_type: Some("Weighted".to_string()), // Assumption for now
            })
            .collect();

        let (token_results, provider) = {
            let mut attempts = 0;
            loop {
                let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
                let multicall = Multicall::new(
                    provider.clone(),
                    self.multicall_address,
                    self.multicall_batch_size,
                );
                let vault_contract = IBalancerV2Vault::new(vault_address, provider.clone());

                let calls: Vec<_> = pools_meta
                    .iter()
                    .filter_map(|pool| {
                        pool.pool_id.map(|id| {
                            let call_data = vault_contract
                                .get_pool_tokens(id.into())
                                .calldata()
                                .unwrap();
                            Call {
                                target: vault_address,
                                call_data,
                            }
                        })
                    })
                    .collect();

                match multicall.run(calls, None).await {
                    Ok(results) => break (results, provider),
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
                                "Failed to get pool tokens after {} attempts: {}",
                                attempts,
                                e
                            ));
                        }
                        warn!("Get pool tokens for {} failed, retrying in {:?}. Attempt {}/{}. Error: {}", self.name(), RETRY_DELAY, attempts, MAX_RETRIES, e);
                        sleep(RETRY_DELAY).await;
                    }
                }
            }
        };

        let vault_contract = IBalancerV2Vault::new(vault_address, provider.clone());
        let get_pool_tokens_fn = vault_contract.abi().function("getPoolTokens")?;
        for (i, pool) in pools_meta.iter_mut().enumerate() {
            if let Some(result_data) = token_results.get(i) {
                if let Ok(decoded) = get_pool_tokens_fn.decode_output(result_data) {
                    if let Some(tokens) = decoded[0].clone().into_array() {
                        if tokens.len() >= 2 {
                            pool.token0 = tokens[0].clone().into_address().unwrap_or_default();
                            pool.token1 = tokens[1].clone().into_address().unwrap_or_default();
                        }
                    }
                }
            }
        }

        pools_meta.retain(|p| p.token0 != Address::zero() && p.token1 != Address::zero());
        info!(
            "Finished refreshing Balancer adapter. Found {} valid pools.",
            pools_meta.len()
        );

        Ok(pools_meta)
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
            let vault_address: Address = BALANCER_VAULT_ADDRESS.parse()?;
            let vault_contract = IBalancerV2Vault::new(vault_address, Arc::clone(&provider));

            let mut calls = Vec::new();
            for pool_meta in pools {
                if let Some(pool_id) = pool_meta.pool_id {
                    let call_data = vault_contract
                        .get_pool_tokens(pool_id.into())
                        .calldata()
                        .unwrap();
                    calls.push(Call {
                        target: vault_address,
                        call_data,
                    });

                    if let Some(pool_type) = &pool_meta.pool_type {
                        if pool_type == "Weighted" {
                            let weighted_pool_contract =
                                IWeightedPool::new(pool_meta.address, Arc::clone(&provider));

                            let weights_calldata = weighted_pool_contract
                                .get_normalized_weights()
                                .calldata()
                                .unwrap();
                            calls.push(Call {
                                target: pool_meta.address,
                                call_data: weights_calldata,
                            });

                            let fee_calldata = weighted_pool_contract
                                .get_swap_fee_percentage()
                                .calldata()
                                .unwrap();
                            calls.push(Call {
                                target: pool_meta.address,
                                call_data: fee_calldata,
                            });
                        }
                    }
                }
            }

            match multicall.run(calls, None).await {
                Ok(results) => {
                    let get_pool_tokens_fn = vault_contract.abi().function("getPoolTokens")?;
                    let dummy_weighted_pool =
                        IWeightedPool::new(Address::zero(), Arc::clone(&provider));
                    let get_weights_fn =
                        dummy_weighted_pool.abi().function("getNormalizedWeights")?;
                    let get_fee_fn = dummy_weighted_pool.abi().function("getSwapFeePercentage")?;

                    let mut fetched_pools = Vec::new();
                    let mut result_index = 0;
                    for pool_meta in pools {
                        if let (Some(pool_id), Some(pool_type)) =
                            (pool_meta.pool_id, &pool_meta.pool_type)
                        {
                            if pool_type == "Weighted" {
                                if let (
                                    Some(pool_tokens_data),
                                    Some(weights_data),
                                    Some(fee_data),
                                ) = (
                                    results.get(result_index),
                                    results.get(result_index + 1),
                                    results.get(result_index + 2),
                                ) {
                                    if let (
                                        Ok(tokens_decoded),
                                        Ok(weights_decoded),
                                        Ok(fee_decoded),
                                    ) = (
                                        get_pool_tokens_fn.decode_output(pool_tokens_data),
                                        get_weights_fn.decode_output(weights_data),
                                        get_fee_fn.decode_output(fee_data),
                                    ) {
                                        if let (
                                            Some(tokens_array),
                                            Some(balances_array),
                                            Some(weights_array),
                                            Some(swap_fee),
                                        ) = (
                                            tokens_decoded[0].clone().into_array(),
                                            tokens_decoded[1].clone().into_array(),
                                            weights_decoded[0].clone().into_array(),
                                            fee_decoded[0].clone().into_uint(),
                                        ) {
                                            let tokens = tokens_array
                                                .into_iter()
                                                .filter_map(|t| t.into_address())
                                                .collect();
                                            let balances = balances_array
                                                .into_iter()
                                                .filter_map(|t| t.into_uint())
                                                .collect();
                                            let weights = weights_array
                                                .into_iter()
                                                .filter_map(|t| t.into_uint())
                                                .collect();

                                            fetched_pools.push(Pool::BalancerWeighted(
                                                BalancerWeightedPool {
                                                    address: pool_meta.address,
                                                    pool_id,
                                                    tokens,
                                                    balances,
                                                    weights,
                                                    swap_fee,
                                                    dex: pool_meta.dex,
                                                },
                                            ));
                                        }
                                    }
                                }
                                result_index += 3;
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
