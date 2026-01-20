pub mod pending;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use ethers::providers::Middleware;
use ethers::types::{Address, H256};
use ethers::utils::keccak256;
use log::{debug, trace, warn};

use crate::dex_adapter::PoolMeta;
use crate::rpc_pool::{RpcPool, RpcRole};
use crate::settings::{Discovery, ProtocolFingerprint};

#[derive(Clone, Debug)]
struct ProtocolSpec {
    name: String,
    dex_label: String,
    aliases: Vec<String>,
    factory_addresses: HashSet<Address>,
    bytecode_hashes: HashSet<H256>,
    init_code_hashes: Vec<H256>,
}

impl ProtocolSpec {
    fn from_parts(name: &str, fingerprint: &ProtocolFingerprint) -> Self {
        let dex_label = fingerprint
            .dex_label
            .clone()
            .unwrap_or_else(|| name.to_string());

        let factory_addresses = fingerprint
            .factories
            .iter()
            .filter_map(|f| match f.parse::<Address>() {
                Ok(addr) => Some(addr),
                Err(err) => {
                    warn!(
                        "⚠️ Invalid factory address '{}' for protocol {}: {}",
                        f, name, err
                    );
                    None
                }
            })
            .collect::<HashSet<_>>();

        let bytecode_hashes = fingerprint
            .bytecode_hashes
            .iter()
            .filter_map(|hash| match parse_h256(hash) {
                Ok(h) => Some(h),
                Err(err) => {
                    warn!(
                        "⚠️ Invalid bytecode hash '{}' for protocol {}: {}",
                        hash, name, err
                    );
                    None
                }
            })
            .collect::<HashSet<_>>();

        let init_code_hashes = fingerprint
            .init_code_hashes
            .iter()
            .filter_map(|hash| match parse_h256(hash) {
                Ok(h) => Some(h),
                Err(err) => {
                    warn!(
                        "⚠️ Invalid init code hash '{}' for protocol {}: {}",
                        hash, name, err
                    );
                    None
                }
            })
            .collect();

        Self {
            name: name.to_string(),
            dex_label,
            aliases: fingerprint.aliases.clone(),
            factory_addresses,
            bytecode_hashes,
            init_code_hashes,
        }
    }

    fn matches_factory(&self, factory: Address) -> bool {
        self.factory_addresses.contains(&factory)
    }

    fn matches_label(&self, label: &str) -> bool {
        let label_lower = label.to_ascii_lowercase();
        let matches_self = self.dex_label.to_ascii_lowercase() == label_lower
            || self.name.to_ascii_lowercase() == label_lower;
        if matches_self {
            return true;
        }
        self.aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase() == label_lower)
    }

    fn first_init_code_hash(&self) -> Option<H256> {
        self.init_code_hashes.first().copied()
    }
}

fn parse_h256(value: &str) -> Result<H256> {
    let normalized = value.trim_start_matches("0x");
    let bytes = hex::decode(normalized)
        .with_context(|| format!("Invalid hex string for H256: {}", value))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "Expected 32 bytes for H256, got {} (value: {})",
            bytes.len(),
            value
        ));
    }
    Ok(H256::from_slice(&bytes))
}

pub async fn classify_pools(
    pools: Vec<PoolMeta>,
    discovery_cfg: &Discovery,
    rpc_pool: Arc<RpcPool>,
) -> Result<Vec<PoolMeta>> {
    if pools.is_empty() || discovery_cfg.protocol_fingerprints.is_empty() {
        return Ok(pools);
    }

    let specs: Vec<ProtocolSpec> = discovery_cfg
        .protocol_fingerprints
        .iter()
        .map(|(name, fp)| ProtocolSpec::from_parts(name, fp))
        .collect();

    if specs.is_empty() {
        return Ok(pools);
    }

    let mut bytecode_index: HashMap<H256, Vec<usize>> = HashMap::new();
    for (idx, spec) in specs.iter().enumerate() {
        for hash in &spec.bytecode_hashes {
            bytecode_index.entry(*hash).or_default().push(idx);
        }
    }

    let mut classified = Vec::with_capacity(pools.len());

    for mut pool in pools {
        let mut candidates: Vec<usize> = Vec::new();

        if let Some(factory) = pool.factory {
            for (idx, spec) in specs.iter().enumerate() {
                if spec.matches_factory(factory) {
                    candidates.push(idx);
                }
            }
        }

        if candidates.is_empty() {
            for (idx, spec) in specs.iter().enumerate() {
                if spec.matches_label(pool.dex) {
                    candidates.push(idx);
                }
            }
        }

        if candidates.is_empty() {
            // Note: bytecode_hash field removed from PoolMeta
            let hash: Option<H256> = None;
            if let Some(hash) = hash {
                if let Some(list) = bytecode_index.get(&hash) {
                    candidates.extend(list);
                }
            }
        }

        if candidates.is_empty() && !specs.is_empty() {
            // As a last resort, compute bytecode hash once to attempt matching.
            if let Some(hash) = ensure_bytecode_hash(&mut pool, rpc_pool.clone()).await? {
                if let Some(list) = bytecode_index.get(&hash) {
                    candidates.extend(list);
                }
            }
        }

        if candidates.is_empty() {
            trace!(
                "No protocol fingerprint match for pool {:?} (dex={})",
                pool.address,
                pool.dex
            );
            classified.push(pool);
            continue;
        }

        // If multiple candidates remained, try to pick the best match by bytecode hash first.
        let mut selected_idx = *candidates.first().unwrap();
        // Note: bytecode_hash field removed from PoolMeta
        // Try to get bytecode hash via ensure_bytecode_hash if needed
        if let Ok(Some(hash)) = ensure_bytecode_hash(&mut pool.clone(), rpc_pool.clone()).await {
            if let Some(list) = bytecode_index.get(&hash) {
                if let Some(idx) = list.first() {
                    selected_idx = *idx;
                }
            }
        }

        let spec = &specs[selected_idx];

        // Ensure bytecode hash is populated for persistence and validation use.
        // Note: bytecode_hash and init_code_hash fields removed from PoolMeta
        // These would need to be stored elsewhere if needed

        // Update pool.dex to classified label if different.
        if pool.dex != spec.dex_label {
            let new_label = spec.dex_label.clone();
            pool.dex = Box::leak(new_label.into_boxed_str());
        }

        debug!(
            "Classified pool {:?} as {} (factory={:?})",
            pool.address, pool.dex, pool.factory
        );

        classified.push(pool);
    }

    Ok(classified)
}

async fn ensure_bytecode_hash(pool: &mut PoolMeta, rpc_pool: Arc<RpcPool>) -> Result<Option<H256>> {
    // Note: bytecode_hash field removed from PoolMeta - always fetch

    let (provider, _permit) = rpc_pool
        .acquire(RpcRole::Discovery)
        .await
        .context("No provider available for bytecode fingerprinting")?;
    let code = provider
        .get_code(pool.address, None)
        .await
        .context("Failed to fetch pool bytecode")?;

    if code.0.is_empty() {
        warn!("Pool {:?} returned empty bytecode", pool.address);
        return Ok(None);
    }

    let hash = H256::from(keccak256(&code.0));
    // Note: bytecode_hash field removed from PoolMeta - cannot assign
    Ok(Some(hash))
}
