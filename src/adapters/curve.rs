use anyhow::Result;
use async_trait::async_trait;
use ethers::prelude::*;
use log::{debug, info, warn};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::contracts::ICurvePool;
use crate::dex_adapter::{DexAdapter, PoolMeta};
use crate::multicall::{Call, Multicall};
use crate::pools::{CurveStableSwapPool, Pool};
use crate::rpc_pool::RpcPool;

// AddressProvider ABI for dynamic MetaRegistry lookup
abigen!(
    CurveAddressProvider,
    r#"[
        function get_address(uint256 id) external view returns (address)
    ]"#,
);

// MetaRegistry ABI (ID=7 from AddressProvider)
abigen!(
    CurveMetaRegistry,
    r#"[
        function pool_count() external view returns (uint256)
        function pool_list(uint256) external view returns (address)
        function get_underlying_coins(address) external view returns (address[8])
        function get_underlying_balances(address) external view returns (uint256[8])
        function get_fees(address) external view returns (uint256[2])
        function find_pools_for_coins(address, address) external view returns (address[] memory)
    ]"#,
);

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

// Curve AddressProvider (same across chains)
const CURVE_ADDRESS_PROVIDER: &str = "0x5ffe7FB82894076ECB99A30D6A32e969e6e35E98";

// MetaRegistry ID in AddressProvider
const META_REGISTRY_ID: u64 = 7;

#[derive(Clone)]
pub struct CurveAdapter {
    rpc_pool: Arc<RpcPool>,
    multicall_address: Address,
    multicall_batch_size: usize,
    last_discovery_timestamp: Arc<AtomicU64>,
}

impl CurveAdapter {
    pub fn new(
        rpc_pool: Arc<RpcPool>,
        multicall_address: Address,
        multicall_batch_size: usize,
    ) -> Self {
        Self {
            rpc_pool,
            multicall_address,
            multicall_batch_size,
            last_discovery_timestamp: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl DexAdapter for CurveAdapter {
    fn name(&self) -> &'static str {
        "Curve"
    }

    async fn discover_pools(
        &self,
        _from_block: u64,
        _to_block: u64,
        _chunk_size: u64,
        _max_concurrency: usize,
    ) -> Result<Vec<PoolMeta>> {
        // 🔧 FIX INFINITE LOOP: Curve uses MetaRegistry (static), not events
        // Only discover if >3 minutes passed since last discovery (prevents loop)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let last = self.last_discovery_timestamp.load(Ordering::Relaxed);

        if now - last < 180 {
            // 3 minutes = background_discoverer interval
            let elapsed = now - last;
            info!("⏭️  Curve MetaRegistry queried {}s ago, skipping to avoid infinite loop (wait {}s more)",
                  elapsed, 180 - elapsed);
            return Ok(Vec::new());
        }

        // Update timestamp
        self.last_discovery_timestamp.store(now, Ordering::Relaxed);

        info!("🔍 Discovering Curve pools via MetaRegistry (first call in this 3-min window)");

        // Step 1: Get MetaRegistry address from AddressProvider
        let (provider, _permit) = self.rpc_pool.get_next_provider().await?;
        let address_provider_addr: Address = CURVE_ADDRESS_PROVIDER.parse()?;
        let address_provider = CurveAddressProvider::new(address_provider_addr, provider.clone());

        info!(
            "📡 Querying AddressProvider at {} for MetaRegistry (ID={})",
            address_provider_addr, META_REGISTRY_ID
        );
        let meta_registry_addr = address_provider
            .get_address(U256::from(META_REGISTRY_ID))
            .call()
            .await?;

        if meta_registry_addr == Address::zero() {
            return Err(anyhow::anyhow!(
                "MetaRegistry not found in AddressProvider (ID={})",
                META_REGISTRY_ID
            ));
        }

        info!("✅ MetaRegistry found at: {}", meta_registry_addr);

        // Step 2: Use MetaRegistry to get pools
        let meta_registry = CurveMetaRegistry::new(meta_registry_addr, provider.clone());

        let pool_count = meta_registry
            .pool_count()
            .call()
            .await?
            .as_u64()
            .min(10_000); // Cap at 10k for safety
        info!("📊 MetaRegistry reports {} pools", pool_count);
        debug!("Found {} pools in Curve registry", pool_count);

        let multicall = Multicall::new(
            provider.clone(),
            self.multicall_address,
            self.multicall_batch_size,
        );

        let calls: Vec<_> = (0..pool_count)
            .map(|i| {
                let call_data = meta_registry.pool_list(i.into()).calldata().unwrap();
                Call {
                    target: meta_registry_addr,
                    call_data,
                }
            })
            .collect();

        let results = multicall.run(calls.clone(), None).await?;

        let pool_list_fn = meta_registry.abi().function("pool_list")?;
        let pool_addresses: Vec<Address> = results
            .into_iter()
            .filter_map(|res| {
                pool_list_fn
                    .decode_output(&res)
                    .ok()
                    .and_then(|tokens| tokens[0].clone().into_address())
                    .filter(|&addr| addr != Address::zero())
            })
            .collect();
        info!(
            "📋 Discovered {} pool addresses from MetaRegistry",
            pool_addresses.len()
        );

        // Batch call to get both tokens and fees
        let mut calls: Vec<_> = pool_addresses
            .iter()
            .map(|&pool_address| {
                let call_data = meta_registry
                    .get_underlying_coins(pool_address)
                    .calldata()
                    .unwrap();
                Call {
                    target: meta_registry_addr,
                    call_data,
                }
            })
            .collect();

        // Add fee calls
        let fee_calls: Vec<_> = pool_addresses
            .iter()
            .map(|&pool_address| {
                let call_data = meta_registry.get_fees(pool_address).calldata().unwrap();
                Call {
                    target: meta_registry_addr,
                    call_data,
                }
            })
            .collect();
        calls.extend(fee_calls);

        let results = multicall.run(calls.clone(), None).await?;
        let num_pools = pool_addresses.len();

        let get_coins_fn = meta_registry.abi().function("get_underlying_coins")?;
        let get_fees_fn = meta_registry.abi().function("get_fees")?;
        let mut discovered_pools = Vec::new();
        let mut decode_errors = 0;
        let mut empty_results = 0;

        for (i, pool_address) in pool_addresses.iter().enumerate() {
            // Get tokens result
            let tokens_result = if let Some(result_data) = results.get(i) {
                if result_data.is_empty() {
                    empty_results += 1;
                    None
                } else {
                    match get_coins_fn.decode_output(result_data) {
                        Ok(decoded) => match decoded[0].clone() {
                            ethers::abi::Token::Array(arr)
                            | ethers::abi::Token::FixedArray(arr) => Some(
                                arr.into_iter()
                                    .filter_map(|t| {
                                        t.into_address().filter(|&a| a != Address::zero())
                                    })
                                    .collect::<Vec<Address>>(),
                            ),
                            _ => {
                                decode_errors += 1;
                                if decode_errors <= 5 {
                                    debug!(
                                        "❌ Pool {}: decoded[0] is not an array. Type: {:?}",
                                        pool_address, decoded[0]
                                    );
                                }
                                None
                            }
                        },
                        Err(e) => {
                            decode_errors += 1;
                            if decode_errors <= 3 {
                                debug!(
                                    "❌ Failed to decode coins for pool {}: {}",
                                    pool_address, e
                                );
                            }
                            None
                        }
                    }
                }
            } else {
                None
            };

            // Get fee result (from second half of results)
            let fee = if let Some(fee_data) = results.get(num_pools + i) {
                if !fee_data.is_empty() {
                    match get_fees_fn.decode_output(fee_data) {
                        Ok(decoded) => {
                            // get_fees returns [uint256, uint256] - [swap_fee, admin_fee]
                            // We want the first one (swap_fee) in basis points
                            if let Some(fee_u256) =
                                decoded.get(0).and_then(|t| t.clone().into_uint())
                            {
                                let fee_bps = (fee_u256.as_u128() / 100_000_000) as u32; // Convert from 1e10 basis to bps
                                Some(fee_bps)
                            } else {
                                debug!(
                                    "⚠️  Pool {}: fee decode returned unexpected type",
                                    pool_address
                                );
                                Some(30) // Default 0.3% for Curve
                            }
                        }
                        Err(_) => {
                            Some(30) // Default 0.3% for Curve
                        }
                    }
                } else {
                    Some(30) // Default 0.3% for Curve
                }
            } else {
                Some(30) // Default 0.3% for Curve
            };

            // Build pool if we have valid tokens
            if let Some(tokens) = tokens_result {
                if tokens.len() >= 2 {
                    discovered_pools.push(PoolMeta {
                        address: *pool_address,
                        factory: Some(meta_registry_addr),
                        pool_id: None,
                        fee,
                        token0: tokens[0],
                        token1: tokens[1],
                        dex: self.name(),
                        pool_type: None,
                    });
                    debug!(
                        "✅ Pool {} has {} tokens, fee: {:?} bps",
                        pool_address,
                        tokens.len(),
                        fee
                    );
                } else {
                    debug!(
                        "⚠️  Pool {} has only {} token(s), skipping",
                        pool_address,
                        tokens.len()
                    );
                }
            }
        }

        if decode_errors > 0 {
            warn!(
                "⚠️  Failed to decode {} pool responses (empty or invalid ABI)",
                decode_errors
            );
        }
        if empty_results > 0 {
            warn!(
                "⚠️  Received {} empty responses from MetaRegistry",
                empty_results
            );
        }

        info!(
            "✅ Finished discovering from Curve MetaRegistry. Found {} valid pools.",
            discovered_pools.len()
        );
        Ok(discovered_pools)
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

            // Get MetaRegistry dynamically
            let address_provider_addr: Address = CURVE_ADDRESS_PROVIDER.parse()?;
            let address_provider =
                CurveAddressProvider::new(address_provider_addr, Arc::clone(&provider));
            let meta_registry_addr = address_provider
                .get_address(U256::from(META_REGISTRY_ID))
                .call()
                .await?;
            let meta_registry = CurveMetaRegistry::new(meta_registry_addr, Arc::clone(&provider));

            let mut calls = Vec::new();
            for pool_meta in pools {
                calls.push(Call {
                    target: pool_meta.address,
                    call_data: ICurvePool::new(pool_meta.address, Arc::clone(&provider))
                        .a()
                        .calldata()
                        .unwrap(),
                });
                calls.push(Call {
                    target: meta_registry_addr,
                    call_data: meta_registry
                        .get_underlying_balances(pool_meta.address)
                        .calldata()
                        .unwrap(),
                });
                calls.push(Call {
                    target: meta_registry_addr,
                    call_data: meta_registry
                        .get_underlying_coins(pool_meta.address)
                        .calldata()
                        .unwrap(),
                });
                calls.push(Call {
                    target: meta_registry_addr,
                    call_data: meta_registry
                        .get_fees(pool_meta.address)
                        .calldata()
                        .unwrap(),
                });
            }

            match multicall.run(calls.clone(), None).await {
                Ok(results) => {
                    let dummy_pool = ICurvePool::new(Address::zero(), Arc::clone(&provider));
                    let a_fn = dummy_pool.abi().function("A")?;
                    let get_balances_fn =
                        meta_registry.abi().function("get_underlying_balances")?;
                    let get_coins_fn = meta_registry.abi().function("get_underlying_coins")?;
                    let get_fees_fn = meta_registry.abi().function("get_fees")?;

                    let mut fetched_pools = Vec::new();
                    for (i, pool_meta) in pools.iter().enumerate() {
                        if let (
                            Some(a_data),
                            Some(balances_data),
                            Some(coins_data),
                            Some(fees_data),
                        ) = (
                            results.get(i * 4),
                            results.get(i * 4 + 1),
                            results.get(i * 4 + 2),
                            results.get(i * 4 + 3),
                        ) {
                            if let (
                                Ok(a_decoded),
                                Ok(balances_decoded),
                                Ok(coins_decoded),
                                Ok(fees_decoded),
                            ) = (
                                a_fn.decode_output(a_data),
                                get_balances_fn.decode_output(balances_data),
                                get_coins_fn.decode_output(coins_data),
                                get_fees_fn.decode_output(fees_data),
                            ) {
                                if let (
                                    Some(a),
                                    Some(raw_balances_array),
                                    Some(raw_coins_array),
                                    Some(fees_array),
                                ) = (
                                    a_decoded[0].clone().into_uint(),
                                    balances_decoded[0].clone().into_array(),
                                    coins_decoded[0].clone().into_array(),
                                    fees_decoded[0].clone().into_array(),
                                ) {
                                    let mut tokens = Vec::new();
                                    let mut balances = Vec::new();
                                    for (j, coin_token) in raw_coins_array.into_iter().enumerate() {
                                        if let Some(coin_addr) = coin_token.into_address() {
                                            if coin_addr != Address::zero() {
                                                tokens.push(coin_addr);
                                                if let Some(bal) = raw_balances_array
                                                    .get(j)
                                                    .and_then(|t| t.clone().into_uint())
                                                {
                                                    balances.push(bal);
                                                }
                                            } else {
                                                break;
                                            }
                                        }
                                    }

                                    let fee = fees_array
                                        .get(0)
                                        .and_then(|f| f.clone().into_uint())
                                        .unwrap_or_default();

                                    if tokens.len() >= 2 {
                                        fetched_pools.push(Pool::CurveStableSwap(
                                            CurveStableSwapPool {
                                                address: pool_meta.address,
                                                tokens,
                                                balances,
                                                a,
                                                fee,
                                                dex: pool_meta.dex,
                                            },
                                        ));
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
