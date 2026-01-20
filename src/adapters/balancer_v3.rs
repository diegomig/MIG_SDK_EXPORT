use async_trait::async_trait;
use ethers::prelude::*;
use anyhow::Result;
use std::sync::Arc;
use log::{info, warn, debug};
use tokio::time::{sleep, Duration};

use crate::dex_adapter::{DexAdapter, PoolMeta};
use crate::pools::{Pool, BalancerWeightedPool};
use crate::multicall::{Multicall, Call};
use crate::contracts::{i_balancer_v2_vault::PoolRegisteredFilter, IBalancerV2Vault};
use crate::rpc_pool::RpcPool;

abigen!(
    IBalancerV3Pool,
    r#"[
        function getSwapFeePercentage() external view returns (uint256)
        function getNormalizedWeights() external view returns (uint256[] memory)
        function getPoolType() external view returns (string memory)
        function isPoolInitialized() external view returns (bool)
    ]"#,
);

// Router V3 ABI - Not currently used, reserved for future optimizations
// abigen!(
//     IBalancerV3Router,
//     r#"[
//         function querySwap(...) external returns (uint256)
//     ]"#,
// );

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

// Balancer V3 uses the SAME Vault as V2 (backward compatible)
const BALANCER_VAULT_ADDRESS: &str = "0xBA12222222228d8Ba445958a75a0704d566BF2C8";

// New V3-specific contracts
const BALANCER_V3_ROUTER: &str = "0xeaedc32a51c510d35ebc11088fd5ff2b47aacf2e";
const BALANCER_V3_BATCH_ROUTER: &str = "0xaD89051bEd8d96f045E8912aE1672c6C0bF8a85E";

#[derive(Clone)]
pub struct BalancerV3Adapter {
    rpc_pool: Arc<RpcPool>,
    multicall_address: Address,
    multicall_batch_size: usize,
}

impl BalancerV3Adapter {
    pub fn new(rpc_pool: Arc<RpcPool>, multicall_address: Address, multicall_batch_size: usize) -> Self {
        Self { rpc_pool, multicall_address, multicall_batch_size }
    }
}

#[async_trait]
impl DexAdapter for BalancerV3Adapter {
    fn name(&self) -> &'static str {
        "BalancerV3"
    }

    async fn discover_pools(
        &self,
        from_block: u64,
        to_block: u64,
        chunk_size: u64,
        _max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>> {
        info!("🔍 Discovering Balancer V3 pools from block {} to {}", from_block, to_block);
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
                    
                    debug!("Querying Balancer V3 PoolRegistered events from block {} to {}", start, end);
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

        // Process chunks sequentially to avoid rate limits
        let mut results: Vec<Vec<PoolRegisteredFilter>> = Vec::new();
        for future in futures_iter {
            // Add delay between chunks to avoid rate limits
            tokio::time::sleep(Duration::from_millis(500)).await;
            
            match future.await {
                Ok(chunk_result) => results.push(chunk_result),
                Err(e) => {
                    warn!("Failed to process Balancer V3 chunk: {}", e);
                    // Continue with next chunk instead of failing completely
                }
            }
        }

        let all_logs: Vec<PoolRegisteredFilter> = results.into_iter().flatten().collect();
        info!("📊 Found {} PoolRegistered events for Balancer V3", all_logs.len());

        let mut pools_meta: Vec<PoolMeta> = all_logs.into_iter().map(|log| PoolMeta {
            address: log.pool_address,
            factory: Some(vault_address),
            pool_id: Some(log.pool_id),
            fee: None,
            token0: Address::zero(),
            token1: Address::zero(),
            dex: self.name(),
            pool_type: Some("V3".to_string()),
        }).collect();

        // Fetch pool tokens using Vault's getPoolTokens
        let (token_results, provider) = {
            let mut attempts = 0;
            loop {
                let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
                let multicall = Multicall::new(provider.clone(), self.multicall_address, self.multicall_batch_size);
                let vault_contract = IBalancerV2Vault::new(vault_address, provider.clone());

                let calls: Vec<_> = pools_meta.iter().filter_map(|pool| {
                    pool.pool_id.map(|id| {
                        let call_data = vault_contract.get_pool_tokens(id.into()).calldata().unwrap();
                        Call { target: vault_address, call_data }
                    })
                }).collect();

                if calls.is_empty() {
                    warn!("⚠️  No pools to fetch tokens for");
                    return Ok(vec![]);
                }

                match multicall.run(calls, None).await {
                    Ok(results) => break (results, provider),
                    Err(e) => {
                        let error_string = e.to_string().to_lowercase();
                        if error_string.contains("429") || error_string.contains("too many requests") || error_string.contains("limit exceeded") {
                            self.rpc_pool.report_rate_limit_error(&provider);
                        } else {
                            self.rpc_pool.mark_as_unhealthy(&provider);
                        }
                        attempts += 1;
                        if attempts >= MAX_RETRIES {
                            return Err(anyhow::anyhow!("Failed to get pool tokens after {} attempts: {}", attempts, e));
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
                            debug!("✅ Pool {} has {} tokens", pool.address, tokens.len());
                        } else {
                            debug!("⚠️  Pool {} has only {} token(s)", pool.address, tokens.len());
                        }
                    }
                }
            }
        }

        // Filter out pools without valid tokens
        pools_meta.retain(|p| p.token0 != Address::zero() && p.token1 != Address::zero());
        info!("✅ Discovered {} valid Balancer V3 pools with tokens", pools_meta.len());

        Ok(pools_meta)
    }

    async fn fetch_pool_state(&self, pools: &[PoolMeta]) -> Result<Vec<Pool>> {
        if pools.is_empty() {
            return Ok(vec![]);
        }

        info!("🔄 Fetching state for {} Balancer V3 pools", pools.len());
        
        let mut attempts = 0;
        loop {
            let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
            let multicall = Multicall::new(Arc::clone(&provider), self.multicall_address, self.multicall_batch_size);
            let vault_address: Address = BALANCER_VAULT_ADDRESS.parse()?;
            let vault_contract = IBalancerV2Vault::new(vault_address, Arc::clone(&provider));

            // For each pool, we need:
            // 1. getPoolTokens (from Vault)
            // 2. getNormalizedWeights (from Pool contract)
            // 3. getSwapFeePercentage (from Pool contract)
            let mut calls = Vec::new();
            for pool_meta in pools {
                if let Some(pool_id) = pool_meta.pool_id {
                    // Call 1: getPoolTokens
                    let call_data = vault_contract.get_pool_tokens(pool_id.into()).calldata().unwrap();
                    calls.push(Call { target: vault_address, call_data });

                    // Call 2 & 3: Pool-specific calls
                    let pool_contract = IBalancerV3Pool::new(pool_meta.address, Arc::clone(&provider));
                    
                    let weights_calldata = pool_contract.get_normalized_weights().calldata().unwrap();
                    calls.push(Call { target: pool_meta.address, call_data: weights_calldata });

                    let fee_calldata = pool_contract.get_swap_fee_percentage().calldata().unwrap();
                    calls.push(Call { target: pool_meta.address, call_data: fee_calldata });
                }
            }

            if calls.is_empty() {
                warn!("⚠️  No valid pool IDs to fetch state for");
                return Ok(vec![]);
            }

            match multicall.run(calls, None).await {
                Ok(results) => {
                    let get_pool_tokens_fn = vault_contract.abi().function("getPoolTokens")?;
                    let dummy_pool = IBalancerV3Pool::new(Address::zero(), Arc::clone(&provider));
                    let get_weights_fn = dummy_pool.abi().function("getNormalizedWeights")?;
                    let get_fee_fn = dummy_pool.abi().function("getSwapFeePercentage")?;

                    let mut fetched_pools = Vec::new();
                    let mut result_index = 0;
                    
                    for pool_meta in pools {
                        if pool_meta.pool_id.is_some() {
                            if let (Some(pool_tokens_data), Some(weights_data), Some(fee_data)) =
                                (results.get(result_index), results.get(result_index + 1), results.get(result_index + 2)) {
                                
                                if let (Ok(tokens_decoded), Ok(weights_decoded), Ok(fee_decoded)) =
                                    (get_pool_tokens_fn.decode_output(pool_tokens_data), 
                                     get_weights_fn.decode_output(weights_data), 
                                     get_fee_fn.decode_output(fee_data)) {
                                    
                                    if let (Some(tokens_array), Some(balances_array), Some(weights_array), Some(swap_fee)) =
                                        (tokens_decoded[0].clone().into_array(), 
                                         tokens_decoded[1].clone().into_array(), 
                                         weights_decoded[0].clone().into_array(), 
                                         fee_decoded[0].clone().into_uint()) {

                                        let tokens = tokens_array.into_iter().filter_map(|t| t.into_address()).collect();
                                        let balances = balances_array.into_iter().filter_map(|t| t.into_uint()).collect();
                                        let weights = weights_array.into_iter().filter_map(|t| t.into_uint()).collect();

                                        fetched_pools.push(Pool::BalancerWeighted(BalancerWeightedPool {
                                            address: pool_meta.address,
                                            pool_id: pool_meta.pool_id.unwrap(),
                                            tokens,
                                            balances,
                                            weights,
                                            swap_fee,
                                            dex: pool_meta.dex,
                                        }));
                                        
                                        debug!("✅ Fetched state for Balancer V3 pool {}", pool_meta.address);
                                    }
                                }
                            }
                            result_index += 3; // Move to next pool's results
                        }
                    }
                    
                    info!("✅ Successfully fetched state for {} Balancer V3 pools", fetched_pools.len());
                    return Ok(fetched_pools);
                },
                Err(e) => {
                    let error_string = e.to_string().to_lowercase();
                    if error_string.contains("429") || error_string.contains("too many requests") || error_string.contains("limit exceeded") {
                        self.rpc_pool.report_rate_limit_error(&provider);
                    } else {
                        self.rpc_pool.mark_as_unhealthy(&provider);
                    }
                    attempts += 1;
                    if attempts >= MAX_RETRIES {
                        return Err(anyhow::anyhow!("Failed to fetch pool state for {} after {} attempts: {}", self.name(), attempts, e));
                    }
                    warn!("Fetch pool state for {} failed, retrying in {:?}. Attempt {}/{}. Error: {}", self.name(), RETRY_DELAY, attempts, MAX_RETRIES, e);
                    sleep(RETRY_DELAY).await;
                }
            }
        }
    }
}

