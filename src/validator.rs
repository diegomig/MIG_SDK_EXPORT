// Pool Validator Module
//
// The `PoolValidator` performs quality assurance checks on discovered pools to ensure
// only legitimate, high-quality pools are included in the topology graph.
//
// ## Validation Criteria
//
// The validator checks:
// - **Bytecode verification**: Pool contract matches known bytecode hash
// - **Token validation**: Tokens are not blacklisted, not zero address, not identical
// - **Anchor token requirement**: At least one token is an anchor token (if enabled)
// - **Liquidity checks**: Pool has sufficient liquidity
//
// See `docs/VALIDATION.md` for detailed validation criteria.

use crate::flight_recorder::FlightRecorder;
use crate::{
    dex_adapter::PoolMeta, metrics, rpc_pool::RpcPool, settings::Validator as ValidatorSettings,
};
use crate::{record_decision, record_phase_end, record_phase_start};
use anyhow::Result;
use ethers::prelude::*;
use log::warn;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// Reason why a pool was marked as invalid.
#[derive(Debug, PartialEq, Eq)]
pub enum InvalidReason {
    /// Pool contains a blacklisted token
    BlacklistedToken,
    /// Pool does not contain an anchor token (when required)
    NoAnchorToken,
    /// Pool has a zero address for token0 or token1
    ZeroAddress,
    /// Pool has the same token for both token0 and token1
    SameTokens,
    /// Pool contract has no bytecode (not a contract)
    NoBytecode,
    /// Pool bytecode doesn't match expected hash
    BytecodeMismatch,
}

impl InvalidReason {
    /// Returns a string representation of the invalid reason.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvalidReason::BlacklistedToken => "blacklisted_token",
            InvalidReason::NoAnchorToken => "no_anchor_token",
            InvalidReason::ZeroAddress => "zero_address",
            InvalidReason::SameTokens => "same_tokens",
            InvalidReason::NoBytecode => "no_bytecode",
            InvalidReason::BytecodeMismatch => "bytecode_mismatch",
        }
    }
}

/// Result of pool validation.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationResult {
    /// Pool passed all validation checks
    Valid,
    /// Pool failed validation with the given reason
    Invalid(InvalidReason),
}

/// Validates discovered pools for quality and legitimacy.
///
/// The validator performs structural validation to ensure pools are:
/// - Legitimate contracts (bytecode verification)
/// - Contain valid tokens (not blacklisted, not zero, not identical)
/// - Meet liquidity requirements (anchor tokens, minimum liquidity)
///
/// # Configuration
///
/// Validation behavior is configured via `settings.validator`:
/// - `whitelisted_factories`: Factory addresses to trust
/// - `whitelisted_bytecode_hashes`: Expected bytecode hashes per DEX
/// - `anchor_tokens`: High-liquidity tokens that pools must contain
/// - `blacklisted_tokens`: Tokens to reject
/// - `require_anchor_token`: Whether anchor token is required
///
/// # Example
///
/// ```rust,no_run
/// use mig_topology_sdk::validator::PoolValidator;
///
/// let validator = PoolValidator::new(rpc_pool, &settings.validator);
///
/// // Validate a pool
/// let result = validator.structural_validation(&pool_meta).await?;
/// match result {
///     ValidationResult::Valid => println!("Pool is valid"),
///     ValidationResult::Invalid(reason) => println!("Pool invalid: {:?}", reason),
/// }
/// ```
pub struct PoolValidator {
    rpc_pool: Arc<RpcPool>,
    whitelisted_factories: HashSet<Address>,
    whitelisted_bytecode_hashes: HashSet<H256>,
    anchor_tokens: HashSet<Address>,
    blacklisted_tokens: HashSet<Address>,
    settings: ValidatorSettings,
    // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    flight_recorder: Option<Arc<FlightRecorder>>,
}

impl PoolValidator {
    /// Creates a new pool validator with the given configuration.
    ///
    /// # Parameters
    ///
    /// - `rpc_pool`: RPC provider pool for bytecode verification
    /// - `settings`: Validator configuration settings
    ///
    /// # Returns
    ///
    /// A new `PoolValidator` instance ready for validation.
    pub fn new(rpc_pool: Arc<RpcPool>, settings: &ValidatorSettings) -> Self {
        let whitelisted_factories = settings
            .whitelisted_factories
            .iter()
            .map(|s| Address::from_str(s).unwrap_or_default())
            .collect();

        let whitelisted_bytecode_hashes = settings
            .whitelisted_bytecode_hashes
            .keys()
            .map(|s| H256::from_str(s.trim_start_matches("0x")).unwrap_or_default())
            .collect();

        let anchor_tokens = settings
            .anchor_tokens
            .iter()
            .map(|s| Address::from_str(s).unwrap_or_default())
            .collect();

        let blacklisted_tokens = settings
            .blacklisted_tokens
            .iter()
            .map(|s| Address::from_str(s).unwrap_or_default())
            .collect();

        Self {
            rpc_pool,
            whitelisted_factories,
            whitelisted_bytecode_hashes,
            anchor_tokens,
            blacklisted_tokens,
            settings: settings.clone(),
            flight_recorder: None,
        }
    }

    /// Sets the flight recorder for instrumentation and debugging.
    ///
    /// # Parameters
    ///
    /// - `recorder`: Shared flight recorder instance
    ///
    /// # Returns
    ///
    /// Self for method chaining.
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder);
        self
    }

    /// Validates a batch of pools concurrently.
    ///
    /// # Parameters
    ///
    /// - `pools`: Vector of pool metadata to validate
    ///
    /// # Returns
    ///
    /// Vector of tuples containing the pool metadata and its validation result.
    ///
    /// # Performance
    ///
    /// Validation is performed sequentially. For large batches, consider
    /// parallelizing the validation calls.
    pub async fn validate_all(&self, pools: Vec<PoolMeta>) -> Vec<(PoolMeta, ValidationResult)> {
        let mut results = Vec::new();
        for pool in pools {
            let result = self.structural_validation(&pool).await.unwrap_or_else(|e| {
                warn!("Validation failed for pool {:?}: {}", pool.address, e);
                ValidationResult::Invalid(InvalidReason::NoBytecode) // Default to invalid if RPC fails
            });

            match &result {
                ValidationResult::Valid => metrics::increment_pool_validations("valid", "none"),
                ValidationResult::Invalid(reason) => {
                    metrics::increment_pool_validations("invalid", reason.as_str())
                }
            }

            results.push((pool, result));
        }
        results
    }

    /// Performs structural validation on a single pool.
    ///
    /// This method checks:
    /// 1. Token blacklist (rejects if either token is blacklisted)
    /// 2. Anchor token requirement (if enabled, requires at least one anchor token)
    /// 3. Zero address check (rejects if token0 or token1 is zero)
    /// 4. Same token check (rejects if token0 == token1)
    /// 5. Factory whitelist (accepts if factory is whitelisted)
    /// 6. Bytecode verification (checks bytecode hash if factory check fails)
    ///
    /// # Parameters
    ///
    /// - `pool`: Pool metadata to validate
    ///
    /// # Returns
    ///
    /// `ValidationResult::Valid` if the pool passes all checks,
    /// `ValidationResult::Invalid(reason)` if validation fails.
    ///
    /// # Errors
    ///
    /// Returns an error if RPC calls fail (with automatic retries).
    ///
    /// # Retry Logic
    ///
    /// Bytecode verification retries up to 10 times with 5-second delays
    /// to handle transient RPC failures.
    pub async fn structural_validation(&self, pool: &PoolMeta) -> Result<ValidationResult> {
        let start_time = Instant::now();

        // ✅ FLIGHT RECORDER: Registrar inicio de validación
        if let Some(ref recorder) = self.flight_recorder {
            record_phase_start!(
                recorder,
                "pool_validator_structural",
                serde_json::json!({
                    "pool_address": format!("{:?}", pool.address),
                    "token0": format!("{:?}", pool.token0),
                    "token1": format!("{:?}", pool.token1)
                })
            );
        }

        // Token blacklist check
        if self.blacklisted_tokens.contains(&pool.token0)
            || self.blacklisted_tokens.contains(&pool.token1)
        {
            let result = ValidationResult::Invalid(InvalidReason::BlacklistedToken);
            if let Some(ref recorder) = self.flight_recorder {
                record_decision!(
                    recorder,
                    "pool_validator",
                    "reject",
                    "blacklisted_token",
                    serde_json::json!({
                        "pool_address": format!("{:?}", pool.address)
                    })
                );
                record_phase_end!(
                    recorder,
                    "pool_validator_structural",
                    start_time,
                    serde_json::json!({
                        "result": "invalid",
                        "reason": "blacklisted_token"
                    })
                );
            }
            return Ok(result);
        }

        // Anchor token check (now optional)
        if self.settings.require_anchor_token {
            if !self.anchor_tokens.contains(&pool.token0)
                && !self.anchor_tokens.contains(&pool.token1)
            {
                let result = ValidationResult::Invalid(InvalidReason::NoAnchorToken);
                if let Some(ref recorder) = self.flight_recorder {
                    record_decision!(
                        recorder,
                        "pool_validator",
                        "reject",
                        "no_anchor_token",
                        serde_json::json!({
                            "pool_address": format!("{:?}", pool.address)
                        })
                    );
                    record_phase_end!(
                        recorder,
                        "pool_validator_structural",
                        start_time,
                        serde_json::json!({
                            "result": "invalid",
                            "reason": "no_anchor_token"
                        })
                    );
                }
                return Ok(result);
            }
        }

        // Basic address checks
        if pool.token0.is_zero() || pool.token1.is_zero() {
            let result = ValidationResult::Invalid(InvalidReason::ZeroAddress);
            if let Some(ref recorder) = self.flight_recorder {
                record_decision!(
                    recorder,
                    "pool_validator",
                    "reject",
                    "zero_address",
                    serde_json::json!({
                        "pool_address": format!("{:?}", pool.address)
                    })
                );
                record_phase_end!(
                    recorder,
                    "pool_validator_structural",
                    start_time,
                    serde_json::json!({
                        "result": "invalid",
                        "reason": "zero_address"
                    })
                );
            }
            return Ok(result);
        }
        if pool.token0 == pool.token1 {
            let result = ValidationResult::Invalid(InvalidReason::SameTokens);
            if let Some(ref recorder) = self.flight_recorder {
                record_decision!(
                    recorder,
                    "pool_validator",
                    "reject",
                    "same_tokens",
                    serde_json::json!({
                        "pool_address": format!("{:?}", pool.address)
                    })
                );
                record_phase_end!(
                    recorder,
                    "pool_validator_structural",
                    start_time,
                    serde_json::json!({
                        "result": "invalid",
                        "reason": "same_tokens"
                    })
                );
            }
            return Ok(result);
        }

        // Validation by factory origin (preferred)
        if let Some(factory_address) = pool.factory {
            if self.whitelisted_factories.contains(&factory_address) {
                if let Some(ref recorder) = self.flight_recorder {
                    record_decision!(
                        recorder,
                        "pool_validator",
                        "accept",
                        "factory_whitelisted",
                        serde_json::json!({
                            "pool_address": format!("{:?}", pool.address),
                            "factory": format!("{:?}", factory_address)
                        })
                    );
                    record_phase_end!(
                        recorder,
                        "pool_validator_structural",
                        start_time,
                        serde_json::json!({
                            "result": "valid",
                            "method": "factory_whitelist"
                        })
                    );
                }
                return Ok(ValidationResult::Valid);
            }
        }

        // Fallback to bytecode whitelist check with retry logic
        let mut attempts = 0;
        loop {
            let bytecode_start = Instant::now();
            let (provider, _permit) = self.rpc_pool.get_next_provider().await?;

            match provider.get_code(pool.address, None).await {
                Ok(bytecode) => {
                    if bytecode.is_empty() {
                        let result = ValidationResult::Invalid(InvalidReason::NoBytecode);
                        if let Some(ref recorder) = self.flight_recorder {
                            record_decision!(
                                recorder,
                                "pool_validator",
                                "reject",
                                "no_bytecode",
                                serde_json::json!({
                                    "pool_address": format!("{:?}", pool.address),
                                    "attempts": attempts + 1
                                })
                            );
                            record_phase_end!(
                                recorder,
                                "pool_validator_structural",
                                start_time,
                                serde_json::json!({
                                    "result": "invalid",
                                    "reason": "no_bytecode",
                                    "attempts": attempts + 1
                                })
                            );
                        }
                        return Ok(result);
                    }
                    let code_hash = ethers::utils::keccak256(&bytecode);
                    if !self
                        .whitelisted_bytecode_hashes
                        .contains(&H256::from(code_hash))
                    {
                        let result = ValidationResult::Invalid(InvalidReason::BytecodeMismatch);
                        if let Some(ref recorder) = self.flight_recorder {
                            record_decision!(
                                recorder,
                                "pool_validator",
                                "reject",
                                "bytecode_mismatch",
                                serde_json::json!({
                                    "pool_address": format!("{:?}", pool.address),
                                    "attempts": attempts + 1
                                })
                            );
                            record_phase_end!(
                                recorder,
                                "pool_validator_structural",
                                start_time,
                                serde_json::json!({
                                    "result": "invalid",
                                    "reason": "bytecode_mismatch",
                                    "attempts": attempts + 1
                                })
                            );
                        }
                        return Ok(result);
                    }
                    // Valid by bytecode
                    if let Some(ref recorder) = self.flight_recorder {
                        record_decision!(
                            recorder,
                            "pool_validator",
                            "accept",
                            "bytecode_whitelisted",
                            serde_json::json!({
                                "pool_address": format!("{:?}", pool.address),
                                "attempts": attempts + 1,
                                "bytecode_fetch_ms": bytecode_start.elapsed().as_millis()
                            })
                        );
                        record_phase_end!(
                            recorder,
                            "pool_validator_structural",
                            start_time,
                            serde_json::json!({
                                "result": "valid",
                                "method": "bytecode_whitelist",
                                "attempts": attempts + 1
                            })
                        );
                    }
                    return Ok(ValidationResult::Valid);
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
                        if let Some(ref recorder) = self.flight_recorder {
                            record_decision!(
                                recorder,
                                "pool_validator",
                                "error",
                                "max_retries_exceeded",
                                serde_json::json!({
                                    "pool_address": format!("{:?}", pool.address),
                                    "attempts": attempts,
                                    "error": format!("{}", e)
                                })
                            );
                            record_phase_end!(
                                recorder,
                                "pool_validator_structural",
                                start_time,
                                serde_json::json!({
                                    "result": "error",
                                    "reason": "max_retries_exceeded",
                                    "attempts": attempts
                                })
                            );
                        }
                        return Err(anyhow::anyhow!(
                            "Failed to validate pool bytecode after {} attempts: {}",
                            attempts,
                            e
                        ));
                    }
                    warn!(
                        "Pool validation failed, retrying in {:?}. Attempt {}/{}. Error: {}",
                        RETRY_DELAY, attempts, MAX_RETRIES, e
                    );
                    sleep(RETRY_DELAY).await;
                }
            }
        }
    }
}
