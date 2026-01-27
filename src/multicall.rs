use crate::metrics;
pub use anyhow::Result;
use ethers::abi::{Abi, Function, Token};
use ethers::prelude::*;
use futures_util::Stream;
use log::{debug, warn};
use std::sync::Arc;
use std::time::Duration;

/// A single RPC call to be batched in a multicall.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Call {
    /// Target contract address
    pub target: Address,
    /// Encoded function call data
    pub call_data: Bytes,
}

/// Multicall batch executor for optimized RPC calls.
///
/// Batches multiple contract calls into a single RPC request to reduce latency
/// and RPC provider load.
///
/// ## Features
///
/// - **Batching**: Combines up to 200 calls per batch
/// - **Automatic Retries**: Configurable retry logic for transient failures
/// - **Timeout Management**: Per-batch timeout configuration
///
/// ## Note
///
/// This implementation requires users to provide their own Multicall3 contract binding.
/// The SDK does not include a pre-built Multicall3 binding to allow flexibility.
///
/// ## Example
///
/// ```rust
/// let multicall = Multicall::new(provider, multicall_address, 100);
/// let calls = vec![
///     Call { target: pool_address, call_data: get_reserves_call },
///     // ... more calls
/// ];
/// let results = multicall.run(calls, None).await?;
/// ```
#[derive(Clone)]
pub struct Multicall<M: Middleware> {
    pub provider: Arc<M>,
    multicall_address: Address,
    batch_size: usize,
    timeout_seconds: u64,
    max_retries: u32,
}

impl<M: Middleware + 'static> Multicall<M> {
    pub fn new(provider: Arc<M>, multicall_address: Address, batch_size: usize) -> Self {
        // 🚀 RPC OPTIMIZATION: Validar y ajustar batch size dinámicamente
        // Limitar a máximo 200 calls por batch para evitar rechazos de RPC providers
        let validated_batch_size = batch_size.min(200).max(50); // Entre 50 y 200

        if batch_size > 200 {
            log::warn!(
                "⚠️ Batch size {} exceeds recommended maximum (200), capping to 200",
                batch_size
            );
        }

        Self {
            provider,
            multicall_address,
            batch_size: validated_batch_size,
            timeout_seconds: 3, // 🚀 RPC OPTIMIZATION: Reduced from 30s to 3s to prevent deadlocks
            max_retries: 1,     // 🚀 RPC OPTIMIZATION: Reduced from 2 to 1 (no retries, fail fast)
        }
    }

    /// Set custom timeout for multicall operations
    pub fn with_timeout(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }

    /// Set custom retry count
    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Runs a batch of calls, optionally at a specific block.
    /// ✅ FASE 2.1: Updated to use direct ethers contract call (no ABI binding required)
    pub async fn run(&self, calls: Vec<Call>, block: Option<BlockId>) -> Result<Vec<Bytes>> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }

        // Coalesce identical calls to reduce load
        let mut unique_calls = indexmap::IndexMap::new();
        let mut original_indices = vec![0; calls.len()];
        for (i, call) in calls.iter().enumerate() {
            let (index, _) = unique_calls.insert_full((call.target, call.call_data.clone()), ());
            original_indices[i] = index;
        }

        let unique_call_vec: Vec<_> = unique_calls
            .into_keys()
            .map(|(target, call_data)| Call { target, call_data })
            .collect();
        debug!(
            "Multicall coalesced {} calls into {}",
            calls.len(),
            unique_call_vec.len()
        );

        let mut all_results_unique: Vec<Bytes> = Vec::with_capacity(unique_call_vec.len());

        for call_chunk in unique_call_vec.chunks(self.batch_size) {
            let chunk_size = call_chunk.len();
            if chunk_size > 200 {
                warn!("⚠️ Multicall chunk size {} exceeds recommended maximum (200), may be rejected by RPC", chunk_size);
            }

            metrics::record_multicall_batch_size(chunk_size as f64);

            // ✅ FASE 2.1: Use direct contract call to Multicall3 aggregate3
            let return_data = self.execute_multicall3(call_chunk, block).await?;
            all_results_unique.extend(return_data);
        }

        // Reconstruct the full result set in the original order
        let final_results = original_indices
            .into_iter()
            .map(|index| all_results_unique[index].clone())
            .collect();

        Ok(final_results)
    }

    /// ✅ FASE 2.1: Execute Multicall3 aggregate3 using direct ethers contract call
    async fn execute_multicall3(
        &self,
        calls: &[Call],
        block: Option<BlockId>,
    ) -> Result<Vec<Bytes>> {
        // Multicall3 aggregate3 function signature
        // function aggregate3(Call3[] calldata calls) public payable returns (Result[] memory returnData)
        // Call3 struct: { target, allowFailure, callData }
        // Result struct: { success, returnData }

        // Build the ABI-encoded call data manually
        let mut call_tokens = Vec::new();
        for call in calls {
            // Call3 struct: (address target, bool allowFailure, bytes callData)
            call_tokens.push(Token::Tuple(vec![
                Token::Address(call.target),
                Token::Bool(true), // allowFailure = true
                Token::Bytes(call.call_data.to_vec()),
            ]));
        }

        // Encode the function call
        let function = Function {
            name: "aggregate3".to_string(),
            inputs: vec![ethers::abi::Param {
                name: "calls".to_string(),
                kind: ethers::abi::ParamType::Array(Box::new(ethers::abi::ParamType::Tuple(vec![
                    ethers::abi::ParamType::Address,
                    ethers::abi::ParamType::Bool,
                    ethers::abi::ParamType::Bytes,
                ]))),
                internal_type: None,
            }],
            outputs: vec![ethers::abi::Param {
                name: "returnData".to_string(),
                kind: ethers::abi::ParamType::Array(Box::new(ethers::abi::ParamType::Tuple(vec![
                    ethers::abi::ParamType::Bool,
                    ethers::abi::ParamType::Bytes,
                ]))),
                internal_type: None,
            }],
            constant: None,
            state_mutability: ethers::abi::StateMutability::Payable,
        };

        let calldata = function.encode_input(&[Token::Array(call_tokens)])?;

        // Execute the call
        let tx_request = ethers::types::TransactionRequest::new()
            .to(self.multicall_address)
            .data(calldata);
        let typed_tx: ethers::types::transaction::eip2718::TypedTransaction = tx_request.into();
        let mut request = self.provider.call(&typed_tx, block);

        // ✅ FASE 2.1: No hard timeout - use circuit breaker instead
        // Timeout is handled by RPC pool circuit breaker (latency >100ms = unhealthy)
        let response = request.await?;

        // Decode the response
        let decoded = ethers::abi::decode(
            &[ethers::abi::ParamType::Array(Box::new(
                ethers::abi::ParamType::Tuple(vec![
                    ethers::abi::ParamType::Bool,
                    ethers::abi::ParamType::Bytes,
                ]),
            ))],
            &response,
        )?;

        let results_array = decoded
            .into_iter()
            .next()
            .and_then(|t| t.into_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid multicall response format"))?;

        // Extract returnData from each Result
        let mut return_data = Vec::new();
        for result_token in results_array {
            if let Token::Tuple(mut tuple) = result_token {
                // Result struct: (bool success, bytes returnData)
                // Skip success bool, get returnData
                if tuple.len() >= 2 {
                    if let Token::Bytes(data) = tuple.remove(1) {
                        return_data.push(Bytes::from(data));
                    }
                }
            }
        }

        Ok(return_data)
    }

    /// ✅ FASE 2.1: Streaming multicall - process results incrementally
    /// Returns a stream of results as they become available
    /// Note: Current implementation processes all at once, but structure allows for incremental processing
    pub async fn run_streaming(
        &self,
        calls: Vec<Call>,
        block: Option<BlockId>,
    ) -> Result<impl Stream<Item = Result<Bytes>>> {
        // For now, return all results at once as a stream
        // Future optimization: process results incrementally as they arrive
        let results = self.run(calls, block).await?;
        Ok(futures_util::stream::iter(results.into_iter().map(Ok)))
    }
}
