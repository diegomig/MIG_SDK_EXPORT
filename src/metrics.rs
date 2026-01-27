// src/metrics.rs

#[cfg(feature = "observability")]
pub use metrics::{
    counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
    increment_counter, Unit,
};

// NOTE: When observability feature is disabled, provide stub implementations
#[cfg(not(feature = "observability"))]
pub enum Unit {}

// Macros for metrics when observability is disabled
#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! counter {
    ($name:expr, $value:expr $(, $label:expr => $label_value:expr)* $(,)?) => {};
    ($name:expr $(, $label:expr => $label_value:expr)* $(,)?) => {};
}

#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! gauge {
    ($name:expr, $value:expr $(, $label:expr => $label_value:expr)* $(,)?) => {};
}

#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! histogram {
    ($name:expr, $value:expr $(, $label:expr => $label_value:expr)* $(,)?) => {};
}

#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! increment_counter {
    ($name:expr $(, $label:expr => $label_value:expr)* $(,)?) => {};
}

// Macros for describe_* functions when observability is disabled
#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! describe_counter {
    ($name:expr, $unit:expr, $desc:expr) => {};
    ($name:expr, $desc:expr) => {};
}

#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! describe_gauge {
    ($name:expr, $desc:expr) => {};
}

#[cfg(not(feature = "observability"))]
#[macro_export]
macro_rules! describe_histogram {
    ($name:expr, $unit:expr, $desc:expr) => {};
    ($name:expr, $desc:expr) => {};
}

// Re-export macros for use in this module when observability is disabled
#[cfg(not(feature = "observability"))]
use crate::{
    counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
    increment_counter,
};

use std::sync::atomic::AtomicU64;

// Sampling counter to reduce pressure on high-frequency histograms (e.g., RPC latency)
static RPC_LATENCY_SAMPLE_COUNTER: AtomicU64 = AtomicU64::new(0);
static STATE_REFRESH_SAMPLE_COUNTER: AtomicU64 = AtomicU64::new(0);
static RPC_CALL_MS_SAMPLE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Initializes the descriptions for all the metrics in the application.
/// This should be called once at startup.
pub fn describe_metrics() {
    // Liveness / heartbeat
    describe_gauge!("bot_up", "Bot process liveness (1=up).");
    describe_gauge!(
        "bot_heartbeat_unix_seconds",
        "Last heartbeat timestamp (unix seconds)."
    );

    // MVP metrics
    describe_counter!(
        "mvp_routes_filtered_total",
        Unit::Count,
        "Total number of routes filtered by MVP restrictions, labeled by reason (token_whitelist, max_hops, etc.)."
    );
    describe_counter!(
        "mvp_reserve_validation_aborts_total",
        Unit::Count,
        "Total number of validations aborted due to reserve validation (reserve change exceeded threshold)."
    );
    describe_counter!(
        "mvp_reserve_validation_success_total",
        Unit::Count,
        "Total number of successful reserve validations."
    );
    // MVP Auto metrics
    describe_counter!(
        "mvp_auto_pairs_built_total",
        Unit::Count,
        "Total number of successful dynamic pair whitelist builds."
    );
    describe_gauge!(
        "mvp_auto_pairs_size",
        "Current size of the dynamic pair whitelist."
    );
    // ✅ OPTIMIZATION: MVP E2E and simulation metrics
    describe_histogram!(
        "mvp_block_e2e_ms",
        "End-to-end block processing latency in milliseconds (target: <200ms)."
    );
    describe_histogram!(
        "mvp_block_price_fetch_ms",
        "Price fetch component latency in milliseconds for block processing."
    );
    describe_histogram!(
        "mvp_block_state_update_ms",
        "Pool state update component latency in milliseconds for block processing."
    );
    describe_histogram!(
        "mvp_block_route_load_ms",
        "Route loading component latency in milliseconds for block processing."
    );
    describe_histogram!(
        "mvp_block_simulation_ms",
        "Route simulation component latency in milliseconds for block processing."
    );
    describe_histogram!(
        "mvp_block_post_processing_ms",
        "Post-processing component latency in milliseconds for block processing."
    );
    describe_histogram!(
        "mvp_route_simulation_ms",
        "Total parallel simulation latency in milliseconds for all routes."
    );
    describe_histogram!(
        "mvp_route_simulation_p50_ms",
        "50th percentile (median) of individual route simulation latency in milliseconds."
    );
    describe_histogram!(
        "mvp_route_simulation_p95_ms",
        "95th percentile of individual route simulation latency in milliseconds."
    );
    describe_histogram!(
        "mvp_route_simulation_p99_ms",
        "99th percentile of individual route simulation latency in milliseconds."
    );
    describe_histogram!(
        "mvp_route_simulation_min_ms",
        "Minimum individual route simulation latency in milliseconds."
    );
    describe_histogram!(
        "mvp_route_simulation_max_ms",
        "Maximum individual route simulation latency in milliseconds."
    );
    describe_counter!(
        "bot_errors_total",
        Unit::Count,
        "Total number of generic errors encountered, labeled by error type."
    );
    describe_counter!(
        "bot_v3_fee_corrections_total",
        Unit::Count,
        "Total number of V3 fee corrections applied (converting bps to absolute fee)."
    );
    describe_counter!(
        "bot_routes_using_stale_pools_total",
        Unit::Count,
        "Total number of routes that use stale pools (fallback mechanism)."
    );
    describe_counter!(
        "bot_rpc_errors_total",
        Unit::Count,
        "Total number of RPC errors, labeled by provider URL."
    );

    // --- FASE 1.2: BlockParser get_logs Metrics ---
    describe_counter!(
        "block_parser_get_logs_total",
        Unit::Count,
        "Total number of get_logs() calls triggered by BlockParser, labeled by trigger reason, touched_count, addresses_count, and block number."
    );

    // ✅ OPTIMIZATION: RPC calls per component metrics
    describe_counter!(
        "rpc_calls_total",
        Unit::Count,
        "Total number of RPC calls made, labeled by component (streaming_discovery, fast_lane, background_validator)."
    );
    describe_gauge!(
        "rpc_calls_per_block",
        "Number of RPC calls made per block, labeled by component."
    );
    describe_histogram!(
        "rpc_call_latency_ms",
        "RPC call latency in milliseconds, labeled by component and call_type."
    );

    // ✅ OPTIMIZATION: Data coherence metrics
    describe_gauge!(
        "data_coherence_active_pools_count",
        "Number of active pools in database (for coherence verification)."
    );
    describe_gauge!(
        "data_coherence_stale_state_count",
        "Number of pools with stale state (>10 minutes old)."
    );
    describe_gauge!(
        "data_coherence_pools_without_weights",
        "Number of active pools without calculated weights."
    );
    describe_gauge!(
        "data_coherence_stale_weights_count",
        "Number of pools with stale weights (>15 minutes old)."
    );
    describe_counter!(
        "data_coherence_check_total",
        Unit::Count,
        "Total number of coherence checks performed, labeled by component and status (ok, warning, error)."
    );

    // ✅ OPTIMIZATION: Cache hit rate metrics
    describe_gauge!(
        "background_validator_cache_hit_rate",
        "Cache hit rate for background validator (0.0-1.0)."
    );
    describe_gauge!(
        "background_validator_cache_size",
        "Current size of background validator cache."
    );
    describe_counter!(
        "bot_transaction_errors_total",
        Unit::Count,
        "Total number of transaction submission failures."
    );
    describe_counter!(
        "bot_quote_cache_hit_total",
        Unit::Count,
        "Total hits for the quote cache, labeled by DEX kind."
    );
    describe_counter!(
        "bot_quote_cache_miss_total",
        Unit::Count,
        "Total misses for the quote cache, labeled by DEX kind."
    );
    describe_gauge!(
        "bot_quote_cache_size",
        "The number of items in the V3 quote cache."
    );

    describe_histogram!(
        "bot_pipeline_duration_seconds",
        Unit::Seconds,
        "Histogram of the end-to-end pipeline execution time."
    );
    describe_histogram!(
        "bot_discovery_duration_seconds",
        Unit::Seconds,
        "Histogram of the pool discovery phase duration."
    );
    describe_histogram!(
        "bot_find_optimal_amount_duration_seconds",
        Unit::Seconds,
        "Histogram of the find_optimal_amount function duration."
    );
    describe_histogram!(
        "bot_gas_cost_deviation_percent",
        Unit::Seconds, // Prometheus doesn't have a native "percent" unit, this is a gauge.
        "Deviation between estimated and actual gas cost, as a percentage."
    );

    // Gauges for discovery/validation and routing
    describe_gauge!(
        "bot_pools_discovered_per_dex",
        "Number of discovered pools by DEX (last run)."
    );
    describe_gauge!(
        "bot_pools_valid_per_dex",
        "Number of valid pools by DEX (after validation, last run)."
    );
    describe_gauge!(
        "bot_pools_loaded_from_cache",
        "Pools loaded from cache in the last run."
    );
    describe_gauge!(
        "bot_pools_fetched_from_network",
        "Pools fetched from network in the last run."
    );
    describe_gauge!(
        "bot_simple_routes_found",
        "Count of simple (2-step) routes found in the last run."
    );
    describe_gauge!(
        "bot_triangular_routes_found",
        "Count of triangular (3-step) routes found in the last run."
    );
    describe_counter!(
        "bot_pools_invalidated_total",
        Unit::Count,
        "Total number of pools invalidated, labeled by reason."
    );
    describe_counter!(
        "bot_opportunities_rejected_total",
        Unit::Count,
        "Total number of opportunities rejected by the risk manager, labeled by reason."
    );

    // Pool filter metrics
    describe_counter!(
        "pool_filter_rejected_total",
        Unit::Count,
        "Total number of pools rejected by professional filters, labeled by reason."
    );
    describe_counter!(
        "pool_filter_passed_total",
        Unit::Count,
        "Total number of pools that passed professional filters."
    );
    describe_counter!(
        "pool_filter_effective_liquidity_too_low_total",
        Unit::Count,
        "Total number of pools rejected due to insufficient effective liquidity."
    );
    describe_counter!(
        "pool_filter_price_deviation_too_high_total",
        Unit::Count,
        "Total number of pools rejected due to excessive price deviation from global reference."
    );
    describe_counter!(
        "pool_filter_stale_data_total",
        Unit::Count,
        "Total number of pools rejected due to stale data (too old)."
    );
    describe_counter!(
        "pool_filter_reserve_too_small_total",
        Unit::Count,
        "Total number of pools rejected due to reserves being too small."
    );
    describe_counter!(
        "data.normalization_anomaly_total",
        Unit::Count,
        "Total number of opportunities with normalization anomalies (ratio > 1e6)."
    );
    describe_counter!(
        "v3_simulation_invalid_state_total",
        Unit::Count,
        "Total number of V3 simulations rejected due to invalid pool state (sqrtPriceX96 out of range or amount_out absurd)."
    );
    describe_counter!(
        "route_rejected_total",
        Unit::Count,
        "Total number of routes rejected after simulation due to unrealistic final results, labeled by reason."
    );
    describe_counter!(
        "bot_circuit_breaker_opened_total",
        Unit::Count,
        "Total number of times a circuit breaker has been opened, labeled by provider."
    );
    describe_gauge!(
        "bot_circuit_breaker_state",
        "The current state of a circuit breaker, labeled by provider (0=Closed, 1=Open, 2=HalfOpen)."
    );
    describe_gauge!(
        "bot_discovery_cycle_total_pools_found",
        "Total number of pools found in the last discovery cycle."
    );
    describe_counter!(
        "background_pool_inactive_total",
        Unit::Count,
        "Total number of pools marked inactive by the background discoverer (labels: reason, dex)."
    );
    describe_counter!(
        "bot_pool_state_updates_total",
        Unit::Count,
        "Total number of pool state updates triggered by the adaptive sampler."
    );
    describe_gauge!(
        "bot_adaptive_concurrency_limit",
        "The adaptive concurrency limit for a given RPC provider."
    );

    // RPC Metrics
    describe_counter!(
        "rpc_requests_total",
        "Total RPC requests, labeled by host and type."
    );
    describe_histogram!(
        "rpc_latency_seconds",
        Unit::Seconds,
        "RPC request latency, labeled by host and type."
    );
    describe_counter!("rpc_429_total", "Total HTTP 429 errors, labeled by host.");
    describe_counter!(
        "rpc_unhealthy_total",
        "Total times an RPC provider was marked as unhealthy, labeled by host."
    );

    // Discovery Metrics
    describe_counter!(
        "discovery_blocks_processed_total",
        "Total blocks processed by the discoverer, labeled by dex."
    );
    describe_counter!(
        "discovery_new_pools_total",
        "Total new pools found by the discoverer, labeled by dex."
    );
    describe_counter!(
        "pool_validations_total",
        "Total pool validations, labeled by result and reason."
    );
    describe_gauge!(
        "active_pools_gauge",
        "Current number of active pools, labeled by dex."
    );

    // Cache Metrics
    describe_counter!(
        "cache_hits_total",
        "Total cache hits, labeled by cache name."
    );
    describe_counter!(
        "cache_miss_total",
        "Total cache misses, labeled by cache name."
    );
    describe_gauge!(
        "cache_size_gauge",
        "Current size of a cache, labeled by cache name."
    );

    // Performance Metrics
    describe_histogram!(
        "multicall_batch_size_bucket",
        "Distribution of multicall batch sizes."
    );
    describe_counter!(
        "multicall_zero_results_total",
        Unit::Count,
        "Number of multicall calls that returned zero results, labeled by provider and attempt."
    );
    describe_counter!(
        "multicall_fetch_failed_total",
        Unit::Count,
        "Number of unified state fetch cycles that exhausted all providers and still returned zero results, labeled by provider."
    );

    // Data quality metrics (new)
    describe_gauge!(
        "multicall_partial_fail_rate",
        "Fraction of multicall subcalls that failed in last fetch (0.0-1.0)."
    );
    describe_gauge!(
        "percent_pools_fresh",
        "Percentage of pools with Fresh state among hot pools (0-100)."
    );
    describe_gauge!(
        "warming_candidates_total",
        "Total number of pools evaluated during warming."
    );
    describe_gauge!(
        "warming_candidates_v2",
        "Number of V2 pools evaluated during warming."
    );
    describe_gauge!(
        "warming_candidates_v3",
        "Number of V3 pools evaluated during warming."
    );
    describe_gauge!(
        "warming_accepted_total",
        "Number of pools accepted into the hot cache warming set."
    );
    describe_gauge!(
        "warming_relaxed_thresholds",
        "Indicator (0/1) whether warming relaxed thresholds were applied."
    );
    describe_gauge!(
        "warming_rejected_total",
        "Number of pools rejected during warming."
    );
    describe_gauge!(
        "discovery_pending_queue_size",
        "Number of pools waiting in the discovery/warming pending queue."
    );

    // ✅ FASE 10: Streaming Discovery Metrics
    describe_counter!(
        "streaming_discovery_blocks_received_total",
        Unit::Count,
        "Total number of blocks received from BlockStream by the discoverer."
    );
    describe_counter!(
        "streaming_discovery_pool_candidates_total",
        Unit::Count,
        "Total number of pool candidates extracted from block events, labeled by priority."
    );
    describe_counter!(
        "streaming_discovery_pools_validated_total",
        Unit::Count,
        "Total number of pools validated in streaming discovery, labeled by result."
    );
    describe_counter!(
        "streaming_discovery_pools_inserted_total",
        Unit::Count,
        "Total number of new pools inserted into database from streaming discovery."
    );
    describe_counter!(
        "streaming_discovery_pools_updated_total",
        Unit::Count,
        "Total number of existing pools updated in database from streaming discovery."
    );
    describe_gauge!(
        "streaming_discovery_deferred_queue_size",
        "Current number of pools waiting in the deferred validation queue."
    );
    describe_counter!(
        "streaming_discovery_deferred_pools_processed_total",
        Unit::Count,
        "Total number of pools processed from the deferred queue, labeled by priority."
    );
    describe_histogram!(
        "streaming_discovery_event_extraction_ms",
        "Time taken to extract pool creation events from a block in milliseconds."
    );
    describe_histogram!(
        "streaming_discovery_validation_ms",
        "Time taken to validate pools in streaming discovery in milliseconds."
    );
    describe_histogram!(
        "streaming_discovery_processing_ms",
        "Time taken to process validated pools (DB insert/update) in milliseconds."
    );

    // ✅ FASE 10: BlockStream Metrics
    describe_counter!(
        "blockstream_blocks_published_total",
        Unit::Count,
        "Total number of blocks published to BlockStream."
    );
    describe_gauge!(
        "blockstream_active_subscribers",
        "Current number of active subscribers to BlockStream."
    );
    describe_counter!(
        "blockstream_lag_events_total",
        Unit::Count,
        "Total number of lag events (subscribers falling behind) in BlockStream."
    );

    describe_counter!(
        "combined_multicall_calls_total",
        Unit::Count,
        "Total number of combined multicall calls (discovery)."
    );
    describe_histogram!(
        "combined_multicall_discovery_calls",
        "Number of discovery calls in combined multicall."
    );
    describe_histogram!(
        "combined_multicall_total_calls",
        "Total number of calls in combined multicall."
    );
    describe_counter!(
        "combined_multicall_split_events_total",
        Unit::Count,
        "Total number of times a combined multicall was split due to call limit."
    );
    describe_gauge!(
        "discovery_scheduler_weight",
        "Adaptive scheduler weight (priority) per DEX."
    );
    describe_gauge!(
        "discovery_scheduler_latency_ms",
        "Smoothed discovery latency per DEX in milliseconds."
    );
    describe_gauge!(
        "discovery_scheduler_failure_streak",
        "Current consecutive discovery failure streak per DEX."
    );
    describe_gauge!(
        "warming_last_duration_seconds",
        "Duration of the last warming run in seconds."
    );
    describe_gauge!(
        "warming_avg_duration_per_pool_seconds",
        "Average time spent per pool during the last warming run (seconds)."
    );
    describe_histogram!(
        "warming_duration_seconds",
        Unit::Seconds,
        "Histogram of total warming durations."
    );
    describe_histogram!(
        "warming_duration_per_pool_seconds",
        Unit::Seconds,
        "Histogram of per-pool warming durations."
    );
    describe_counter!(
        "warming_rejected_reason_total",
        Unit::Count,
        "Total number of pools rejected during warming, labeled by reason."
    );
    describe_counter!(
        "warming_rejected_reason_dex_total",
        Unit::Count,
        "Total number of pools rejected during warming, labeled by reason and dex."
    );

    // --- Metrics from new plan ---
    describe_gauge!(
        "db_pools_valid_per_dex",
        "Number of pools marked as is_valid=true in the database, labeled by DEX."
    );
    describe_counter!(
        "bot_routes_found_total",
        Unit::Count,
        "Total valid routes found, labeled by kind (direct/indirect)."
    );
    describe_gauge!(
        "bot_routes_valid_last_block",
        "Number of valid routes in the last block, labeled by kind (direct/indirect)."
    );
    describe_counter!(
        "bot_opportunities_valid_total",
        Unit::Count,
        "Total opportunities that passed simulation and risk checks, labeled by kind (direct/indirect)."
    );
    describe_gauge!(
        "bot_opportunities_valid_last_block",
        "Number of valid opportunities in the last block, labeled by kind (direct/indirect)."
    );
    describe_gauge!(
        "hot_detector_pools_scanned",
        "Number of hot pools scanned during the last detection run."
    );
    describe_gauge!(
        "hot_detector_pools_scanned_per_dex",
        "Number of hot pools scanned in the last detection run, labeled by dex."
    );
    describe_histogram!(
        "hot_detector_detection_ms",
        Unit::Milliseconds,
        "Detection latency for hot pool scanning (milliseconds)."
    );
    describe_counter!(
        "hot_detector_cache_hit_total",
        Unit::Count,
        "Total hot detector cache hits."
    );
    describe_counter!(
        "hot_detector_cache_miss_total",
        Unit::Count,
        "Total hot detector cache misses."
    );
    describe_gauge!(
        "hot_detector_pool_groups",
        "Number of hot pool groups (dex/fee) considered in the last detection run."
    );

    // PHASE 2: Local simulator metrics
    describe_histogram!(
        "sim_local_deviation_bps",
        "Deviation in basis points between local simulator and RPC result, labeled by pool_type."
    );
    describe_counter!(
        "sim_local_hit_total",
        Unit::Count,
        "Total successful local simulator calculations, labeled by pool_type (curve/balancer)."
    );
    describe_counter!(
        "sim_local_error_total",
        Unit::Count,
        "Total local simulator calculation errors, labeled by pool_type."
    );
    describe_counter!(
        "sim_rpc_fallback_total",
        Unit::Count,
        "Total RPC fallbacks when local simulator unavailable, labeled by pool_type."
    );
    describe_counter!(
        "sim_shadow_mismatch_total",
        Unit::Count,
        "Total shadow mode mismatches (deviation > threshold), labeled by pool_type."
    );

    describe_histogram!(
        "bot_wrapper_hops",
        Unit::Count,
        "Number of hops used in indirect route wrappers."
    );
    describe_histogram!(
        "bot_wrapper_cost_usd",
        Unit::Count,
        "Estimated cost of the wrapper swaps in USD."
    );

    // P0.2: Nonce manager metrics
    describe_gauge!(
        "nonce_local_pending",
        "Local pending nonce (next nonce to be used)."
    );
    describe_gauge!(
        "pending_tx_count",
        "Number of pending transactions being tracked."
    );
    describe_counter!(
        "nonce_mismatch_count",
        Unit::Count,
        "Total number of nonce mismatches detected during reconciliation."
    );
    describe_counter!(
        "tx_replacements_total",
        Unit::Count,
        "Total number of transaction replacements (RBF-like)."
    );
    describe_counter!(
        "pending_tx_replacements_total",
        Unit::Count,
        "Total number of pending transaction replacements."
    );
    describe_counter!(
        "pending_tx_confirmed_total",
        Unit::Count,
        "Total number of pending transactions confirmed."
    );
    describe_counter!(
        "pending_tx_dropped_total",
        Unit::Count,
        "Total number of pending transactions dropped."
    );

    // P0.3: Light node metrics
    describe_histogram!(
        "rpc_latency_ms",
        "RPC call latency in milliseconds, labeled by provider."
    );
    describe_counter!(
        "rpc_health_check_failures_total",
        Unit::Count,
        "Total number of RPC health check failures, labeled by provider."
    );

    // P1.2: Métricas específicas del plan
    describe_histogram!(
        "end_to_end_latency_seconds",
        "End-to-end latency in seconds, labeled by stage."
    );
    describe_histogram!(
        "simulation_duration_ms",
        "Simulation duration in milliseconds (alias for bot_simulation_duration_seconds)."
    );
    describe_histogram!(
        "rpc_call_duration_ms",
        "RPC call duration in milliseconds, labeled by method (eth_blockNumber, eth_call, etc.)."
    );
    describe_counter!(
        "price_cache_hits_total",
        Unit::Count,
        "Total number of price cache hits."
    );
    describe_counter!(
        "price_cache_misses_total",
        Unit::Count,
        "Total number of price cache misses."
    );
    describe_counter!(
        "v3_local_simulator_hits_total",
        Unit::Count,
        "Total number of successful V3 local simulator calculations (no RPC fallback)."
    );
    describe_counter!(
        "v3_rpc_fallback_total",
        Unit::Count,
        "Total number of V3 RPC fallbacks when local simulator unavailable or failed."
    );
    describe_gauge!(
        "hot_pools_fresh_count",
        "Number of hot pools with fresh state quality."
    );
    describe_gauge!(
        "hot_pools_stale_count",
        "Number of hot pools with stale state quality."
    );
    describe_counter!(
        "opportunities_failed_total",
        Unit::Count,
        "Total number of opportunities that failed, labeled by reason."
    );
    describe_histogram!(
        "pipeline_parallelism_ratio",
        "Ratio of parallel operations in pipeline (0.0 = fully sequential, 1.0 = fully parallel)."
    );
    describe_histogram!(
        "state_fetch_duration_ms",
        "State fetch duration in milliseconds."
    );
    describe_histogram!(
        "route_discovery_duration_ms",
        "Route discovery duration in milliseconds."
    );
    describe_histogram!(
        "v3_local_sim_deviation_bps",
        "Deviation in basis points between local V3 simulator and QuoterV2, for validation."
    );

    // Pricing fallback metrics
    describe_counter!(
        "price_fallback_used_total",
        Unit::Count,
        "Total times in-memory price fallback was used (0–2 hops)."
    );
    describe_counter!(
        "price_fallback_rejected_by_dev_bps_total",
        Unit::Count,
        "Total times price fallback was rejected due to deviation over tolerance (bps)."
    );
    describe_histogram!(
        "discovery_chunk_duration_seconds",
        Unit::Seconds,
        "Duration of processing a discovery chunk, labeled by DEX."
    );
    describe_counter!(
        "price_feed_failure_total",
        Unit::Count,
        "Total number of failures from the price feed oracle, labeled by type."
    );
    describe_gauge!(
        "oracle_vs_dex_deviation_percent",
        "Price deviation between oracle and DEX, as a percentage."
    );
    describe_histogram!(
        "db_query_duration_seconds",
        Unit::Seconds,
        "Duration of database queries, labeled by operation."
    );

    // --- WebSocket/HTTP ingestion metrics ---
    describe_counter!(
        "ws_subscription_confirmed_total",
        Unit::Count,
        "Total successful WS subscription confirmations."
    );
    describe_gauge!(
        "ws_connected",
        "WebSocket connectivity state (1=connected, 0=disconnected)."
    );
    describe_gauge!(
        "ws_last_activity_unix_seconds",
        "Unix timestamp of last WS activity (message/block)."
    );
    describe_counter!(
        "ws_blocks_received_total",
        Unit::Count,
        "Total blocks received via WebSocket."
    );
    describe_counter!(
        "ws_duplicates_skipped_total",
        Unit::Count,
        "Total duplicate/older WS blocks skipped."
    );
    describe_counter!(
        "ws_unexpected_messages_total",
        Unit::Count,
        "Total unexpected WS messages encountered."
    );
    describe_counter!("ws_errors_total", Unit::Count, "Total WebSocket errors.");
    describe_counter!(
        "ws_disconnects_total",
        Unit::Count,
        "Total WebSocket stream disconnects."
    );
    describe_counter!(
        "ws_fallback_activated_total",
        Unit::Count,
        "Total times HTTP fallback activated due to WS inactivity >=60s."
    );
    describe_gauge!("ws_last_block", "Last block number received via WebSocket.");
    describe_counter!(
        "http_blocks_received_total",
        Unit::Count,
        "Total blocks received via HTTP polling fallback."
    );
    describe_counter!(
        "http_poll_errors_total",
        Unit::Count,
        "Total errors during HTTP block polling."
    );
    describe_counter!(
        "http_poll_provider_unavailable_total",
        Unit::Count,
        "Total times no HTTP provider was available for polling."
    );
    describe_gauge!(
        "http_last_block",
        "Last block number received via HTTP polling."
    );

    // --- Adaptive sizing metrics ---
    describe_histogram!(
        "sizing_selected_amount_usd",
        "Selected amount in USD after applying adaptive sizing."
    );
    describe_counter!(
        "sizing_clamped_total",
        Unit::Count,
        "Number of times adaptive sizing was clamped to min/max bounds."
    );
    describe_counter!(
        "sizing_rule_usage_total",
        Unit::Count,
        "Count of adaptive sizing rule usage, labeled by rule."
    );
    describe_histogram!(
        "sizing_opt_search_latency_ms",
        "Latency (ms) of the optimal sizing search pass."
    );
    describe_histogram!(
        "sizing_opt_search_points",
        "Number of candidate points evaluated during optimal sizing search."
    );
    describe_counter!(
        "sizing_opt_search_fallback_total",
        Unit::Count,
        "Total times the optimal sizing search fell back to heuristics."
    );

    // --- FASE 0: Local V3 Simulator Metrics ---
    describe_counter!(
        "simulator_local_calls_total",
        Unit::Count,
        "Total number of local V3 simulator calls (no RPC)."
    );
    describe_counter!(
        "simulator_local_fallback_rpc_total",
        Unit::Count,
        "Total number of times local simulator fell back to RPC."
    );
    describe_gauge!(
        "simulator_local_match_rate",
        "Percentage of local simulator results that match RPC (shadow mode, 0-100)."
    );
    describe_histogram!(
        "simulator_local_deviation_bps",
        "Deviation between local and RPC results in basis points (shadow mode)."
    );

    // FASE 4: Debug metrics for V3 pool refresh and filtering
    describe_counter!(
        "v3_pools_refresh_attempted_total",
        Unit::Count,
        "Total number of V3 pools attempted to refresh."
    );
    describe_counter!(
        "v3_pools_refresh_succeeded_total",
        Unit::Count,
        "Total number of V3 pools successfully refreshed."
    );
    describe_counter!(
        "v3_pools_refresh_failed_total",
        Unit::Count,
        "Total number of V3 pools that failed to refresh, labeled by reason."
    );
    describe_counter!(
        "v3_pools_filtered_by_stale_total",
        Unit::Count,
        "Total number of V3 pools filtered out due to stale state."
    );
    describe_counter!(
        "v3_pools_filtered_by_dead_total",
        Unit::Count,
        "Total number of V3 pools filtered out due to dead state (zero liquidity or price)."
    );
    describe_counter!(
        "v3_pools_filtered_by_invalid_fee_total",
        Unit::Count,
        "Total number of V3 pools filtered out due to invalid fee tier."
    );
    describe_gauge!(
        "v3_pools_state_quality_fresh",
        "Number of V3 pools with Fresh state quality."
    );
    describe_gauge!(
        "v3_pools_state_quality_stale",
        "Number of V3 pools with Stale state quality."
    );

    // --- FASE 3: Wrapper Discovery Metrics ---
    describe_histogram!(
        "bot_wrapper_discovery_duration_seconds",
        Unit::Seconds,
        "Duration of wrapper discovery phase (finding paths from loan tokens to entry tokens)."
    );
    describe_counter!(
        "bot_wrapper_cache_hits_total",
        Unit::Count,
        "Total number of wrapper path cache hits."
    );
    describe_counter!(
        "bot_wrapper_cache_misses_total",
        Unit::Count,
        "Total number of wrapper path cache misses."
    );

    // --- FASE 3: Redis Metrics ---
    describe_counter!(
        "redis_operations_total",
        Unit::Count,
        "Total number of Redis operations, labeled by operation type."
    );
    describe_histogram!(
        "redis_operation_duration_seconds",
        Unit::Seconds,
        "Duration of Redis operations, labeled by operation type."
    );
    describe_counter!(
        "redis_connection_errors_total",
        Unit::Count,
        "Total number of Redis connection errors."
    );
    describe_counter!(
        "redis_cache_hits_total",
        Unit::Count,
        "Total number of Redis cache hits."
    );
    describe_counter!(
        "redis_cache_misses_total",
        Unit::Count,
        "Total number of Redis cache misses."
    );

    // --- FASE 3: Database Metrics ---
    describe_counter!(
        "bot_db_operations_total",
        Unit::Count,
        "Total number of database operations, labeled by operation type."
    );
    describe_histogram!(
        "bot_db_operation_duration_seconds",
        Unit::Seconds,
        "Duration of database operations, labeled by operation type."
    );
    describe_counter!(
        "bot_db_connection_errors_total",
        Unit::Count,
        "Total number of database connection errors."
    );
    describe_gauge!(
        "bot_db_connections_active",
        "Number of active database connections."
    );
    describe_gauge!(
        "bot_db_connections_idle",
        "Number of idle database connections."
    );
}

// --- Helper functions to update metrics ---

pub fn record_rpc_request(host: &str, req_type: &str, duration: std::time::Duration) {
    counter!("rpc_requests_total", 1, "host" => host.to_string(), "type" => req_type.to_string());
    // Sample histogram to reduce exporter pressure (expensive with high cardinality)
    // Temporarily disabled histogram to prevent exporter hangs
    // const SAMPLE_RATE: u64 = 128; // record 1 out of 128 events (very aggressive)
    // let c = RPC_LATENCY_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // if c % SAMPLE_RATE == 0 {
    //     histogram!(
    //         "rpc_latency_seconds",
    //         duration.as_secs_f64(),
    //         "host" => host.to_string(),
    //         "type" => req_type.to_string()
    //     );
    // }
}

pub fn increment_rpc_429(host: &str) {
    counter!("rpc_429_total", 1, "host" => host.to_string());
}

pub fn increment_rpc_unhealthy(host: &str) {
    counter!("rpc_unhealthy_total", 1, "host" => host.to_string());
}

pub fn increment_discovery_blocks(dex: &str, count: u64) {
    counter!("discovery_blocks_processed_total", count, "dex" => dex.to_string());
}

pub fn increment_new_pools(dex: &str, count: u64) {
    counter!("discovery_new_pools_total", count, "dex" => dex.to_string());
}

pub fn increment_pool_validations(result: &str, reason: &str) {
    counter!("pool_validations_total", 1, "result" => result.to_string(), "reason" => reason.to_string());
}

pub fn set_active_pools(dex: &str, count: f64) {
    gauge!("active_pools_gauge", count, "dex" => dex.to_string());
}

pub fn increment_cache_hit(cache_name: &str) {
    counter!("cache_hits_total", 1, "cache" => cache_name.to_string());
}

pub fn increment_cache_miss(cache_name: &str) {
    counter!("cache_miss_total", 1, "cache" => cache_name.to_string());
}

pub fn set_cache_size(cache_name: &str, size: f64) {
    gauge!("cache_size_gauge", size, "cache" => cache_name.to_string());
}

pub fn record_multicall_batch_size(size: f64) {
    histogram!("multicall_batch_size_bucket", size);
}

pub fn increment_multicall_zero_results(provider: &str, attempt: usize) {
    counter!(
        "multicall_zero_results_total",
        1,
        "provider" => provider.to_string(),
        "attempt" => attempt.to_string()
    );
}

pub fn increment_multicall_fetch_failed(provider: &str) {
    counter!(
        "multicall_fetch_failed_total",
        1,
        "provider" => provider.to_string()
    );
}

pub fn record_sizing_opt_search(duration: std::time::Duration, evaluated_points: usize) {
    histogram!(
        "sizing_opt_search_latency_ms",
        duration.as_secs_f64() * 1000.0
    );
    histogram!("sizing_opt_search_points", evaluated_points as f64);
}

pub fn increment_sizing_opt_search_fallback(reason: &str) {
    counter!(
        "sizing_opt_search_fallback_total",
        1,
        "reason" => reason.to_string()
    );
}

pub fn set_multicall_partial_fail_rate(rate: f64) {
    gauge!("multicall_partial_fail_rate", rate);
}

pub fn set_percent_pools_fresh(percent: f64) {
    gauge!("percent_pools_fresh", percent);
}

pub fn set_adaptive_concurrency(provider_url: &str, value: f64) {
    gauge!("bot_adaptive_concurrency_limit", value, "provider" => provider_url.to_string());
}

pub fn increment_pool_state_updates(count: u64) {
    counter!("bot_pool_state_updates_total", count);
}

pub fn increment_quote_cache_hit(dex_kind: &'static str) {
    counter!("bot_quote_cache_hit_total", 1, "dex_kind" => dex_kind);
}

pub fn increment_quote_cache_miss(dex_kind: &'static str) {
    counter!("bot_quote_cache_miss_total", 1, "dex_kind" => dex_kind);
}

pub fn set_quote_cache_size(size: f64) {
    gauge!("bot_quote_cache_size", size);
}

pub fn increment_errors(error_type: &'static str) {
    counter!("bot_errors_total", 1, "type" => error_type);
}

pub fn increment_rpc_errors(provider_url: &'static str) {
    counter!("bot_rpc_errors_total", 1, "provider" => provider_url);
}

pub fn increment_transaction_errors(op_id: &'static str) {
    counter!("bot_transaction_errors_total", 1, "op_id" => op_id);
}

pub fn increment_invalidated_pools(reason: &'static str) {
    counter!("bot_pools_invalidated_total", 1, "reason" => reason);
}

pub fn increment_circuit_breaker_opened(provider_url: &str) {
    counter!("bot_circuit_breaker_opened_total", 1, "provider" => provider_url.to_string());
}

pub fn set_circuit_breaker_state(provider_url: &str, state: f64) {
    gauge!("bot_circuit_breaker_state", state, "provider" => provider_url.to_string());
}

pub fn record_pipeline_duration(duration: std::time::Duration) {
    histogram!("bot_pipeline_duration_seconds", duration.as_secs_f64());
}

pub fn record_discovery_duration(duration: std::time::Duration) {
    histogram!("bot_discovery_duration_seconds", duration.as_secs_f64());
    // FASE 7.1: Alias for plan compatibility
    histogram!("route_discovery_duration_seconds", duration.as_secs_f64());
}

pub fn record_simulation_duration(duration: std::time::Duration) {
    histogram!("bot_simulation_duration_seconds", duration.as_secs_f64());
    // FASE 7.1: Alias for plan compatibility
    histogram!("simulation_duration_seconds", duration.as_secs_f64());
}

pub fn record_find_optimal_amount_duration(duration: std::time::Duration) {
    histogram!(
        "bot_find_optimal_amount_duration_seconds",
        duration.as_secs_f64()
    );
}

// === PHASE-SPECIFIC METRICS FOR HFT OPTIMIZATION ===

pub fn record_detection_phase_duration(duration: std::time::Duration) {
    histogram!(
        "bot_detection_phase_duration_ms",
        duration.as_millis() as f64
    );
}

pub fn record_state_refresh_duration(duration: std::time::Duration) {
    // Sample to reduce exporter load (state refresh is very frequent)
    // Temporarily disabled histogram to prevent exporter hangs
    // const SAMPLE_RATE: u64 = 100; // record 1 out of 100 events (very aggressive)
    // let c = STATE_REFRESH_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // if c % SAMPLE_RATE == 0 {
    //     histogram!("bot_state_refresh_duration_ms", duration.as_millis() as f64);
    //     // FASE 7.1: Alias for plan compatibility
    //     histogram!("state_fetch_duration_seconds", duration.as_secs_f64());
    // }
}

pub fn record_pricing_phase_duration(duration: std::time::Duration) {
    histogram!("bot_pricing_phase_duration_ms", duration.as_millis() as f64);
}

pub fn record_simulation_phase_duration(duration: std::time::Duration) {
    histogram!(
        "bot_simulation_phase_duration_ms",
        duration.as_millis() as f64
    );
}

pub fn record_submit_phase_duration(duration: std::time::Duration) {
    histogram!("bot_submit_phase_duration_ms", duration.as_millis() as f64);
}

pub fn record_multicall_batch_size_new(size: usize) {
    histogram!("bot_multicall_batch_size", size as f64);
}

pub fn gauge_hot_pools_count(v3_count: usize, v2_count: usize) {
    gauge!("bot_hot_pools_v3_count", v3_count as f64);
    gauge!("bot_hot_pools_v2_count", v2_count as f64);
}

pub fn gauge_all_hot_pools_count(
    v3_count: usize,
    v2_count: usize,
    curve_count: usize,
    balancer_count: usize,
) {
    gauge!("bot_hot_pools_v3_count", v3_count as f64);
    gauge!("bot_hot_pools_v2_count", v2_count as f64);
    gauge!("bot_hot_pools_curve_count", curve_count as f64);
    gauge!("bot_hot_pools_balancer_count", balancer_count as f64);
    gauge!(
        "bot_hot_pools_total_count",
        (v3_count + v2_count + curve_count + balancer_count) as f64
    );
}

pub fn gauge_adaptive_concurrency_limit(host: &str, limit: f64) {
    gauge!("bot_adaptive_concurrency_limit", limit, "host" => host.to_string());
}

pub fn increment_v3_direct_hits() {
    counter!("bot_v3_direct_calculation_hits_total", 1);
}

pub fn increment_quoter_fallbacks() {
    counter!("bot_quoter_fallback_calls_total", 1);
}

pub fn record_gas_cost_deviation(deviation_percent: f64) {
    histogram!("bot_gas_cost_deviation_percent", deviation_percent);
}

pub fn record_db_batch_duration(duration: std::time::Duration) {
    histogram!("bot_db_batch_duration_ms", duration.as_millis() as f64);
}

pub fn record_db_batch_size(size: usize) {
    histogram!("bot_db_batch_size", size as f64);
}

pub fn increment_transactions_submitted(path: &str) {
    counter!("bot_transactions_submitted_total", 1, "path" => path.to_string());
}

pub fn increment_submission_failures(path: &str) {
    counter!("bot_submission_failures_total", 1, "path" => path.to_string());
}

/// Record submission latency for RPC optimization
pub fn record_submission_latency(latency_ms: f64) {
    histogram!("rpc_submission_latency_ms", latency_ms);
}

pub fn record_gas_cost_usd(cost_usd: f64) {
    histogram!("bot_gas_cost_usd_bucket", cost_usd);
}

pub fn record_preparation_phase_duration(duration_ms: f64) {
    histogram!("pipeline_preparation_duration_ms", duration_ms);
}

pub fn record_submission_phase_duration(duration_ms: f64) {
    histogram!("pipeline_submission_duration_ms", duration_ms);
}

pub fn record_pipeline_total_duration(duration_ms: f64) {
    histogram!("pipeline_total_duration_ms", duration_ms);
}

// --- Gauges & heartbeat ---

pub fn record_heartbeat() {
    // Mark process as up and set last-seen timestamp
    gauge!("bot_up", 1.0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    gauge!("bot_heartbeat_unix_seconds", ts);
}

pub fn set_discovered_pools_per_dex(dex: &str, value: f64) {
    gauge!("bot_pools_discovered_per_dex", value, "dex" => dex.to_string());
}

pub fn set_valid_pools_per_dex(dex: &str, value: f64) {
    gauge!("bot_pools_valid_per_dex", value, "dex" => dex.to_string());
}

pub fn set_pools_loaded_from_cache(value: f64) {
    gauge!("bot_pools_loaded_from_cache", value);
}

pub fn set_pools_fetched_from_network(value: f64) {
    gauge!("bot_pools_fetched_from_network", value);
}

pub fn set_simple_routes_found(value: f64) {
    gauge!("bot_simple_routes_found", value);
}

pub fn set_triangular_routes_found(value: f64) {
    gauge!("bot_triangular_routes_found", value);
}

pub fn set_discovery_cycle_total_pools_found(value: f64) {
    gauge!("bot_discovery_cycle_total_pools_found", value);
}

// --- Helpers for new metrics ---

pub fn set_db_pools_valid_per_dex(dex: &str, count: f64) {
    gauge!("db_pools_valid_per_dex", count, "dex" => dex.to_string());
}

pub fn increment_routes_found(kind: &str) {
    counter!("bot_routes_found_total", 1, "kind" => kind.to_string());
}

pub fn set_routes_valid_last_block(kind: &str, count: f64) {
    gauge!("bot_routes_valid_last_block", count, "kind" => kind.to_string());
}

pub fn record_wrapper_hops(hops: f64) {
    histogram!("bot_wrapper_hops", hops);
}

pub fn record_wrapper_cost_usd(cost: f64) {
    histogram!("bot_wrapper_cost_usd", cost);
}

pub fn record_discovery_chunk_duration(dex: &str, duration: std::time::Duration) {
    histogram!("discovery_chunk_duration_seconds", duration.as_secs_f64(), "dex" => dex.to_string());
}

pub fn increment_price_feed_failures(failure_type: &str) {
    counter!("price_feed_failure_total", 1, "type" => failure_type.to_string());
}

pub fn set_oracle_dex_deviation(deviation: f64) {
    gauge!("oracle_vs_dex_deviation_percent", deviation);
}

pub fn record_db_query_duration(operation: &str, duration: std::time::Duration) {
    histogram!("db_query_duration_seconds", duration.as_secs_f64(), "op" => operation.to_string());
}

// === PHASE 2: LOCAL SIMULATOR METRICS ===

/// Generic counter increment for named metrics
/// Note: For performance, prefer specific counter functions below
pub fn increment_counter_named(name: String) {
    counter!(name, 1);
}

pub fn record_histogram_named(name: String, value: f64) {
    histogram!(name, value);
}

pub fn set_gauge_named(name: String, value: f64) {
    gauge!(name, value);
}

/// Record shadow mode deviation between local simulator and RPC result
// FASE 1: Metrics for state synchronization
pub fn increment_simulation_rejected_stale_state() {
    counter!("simulation_rejected_stale_state_total", 1);
}

// FASE 2: Metrics for shadow check optimization
pub fn increment_shadow_check_hops_skipped() {
    counter!("shadow_check_hops_skipped_total", 1);
}

// FASE 3: Metrics for sizing optimization
pub fn increment_sizing_iterations_reduced() {
    counter!("sizing_iterations_reduced_total", 1);
}

// FASE 4: Metrics for price fetch parallelization
pub fn record_price_fetch_parallelization_ms(duration_ms: f64) {
    histogram!("price_fetch_parallelization_ms", duration_ms);
}

// P1.2: Helper functions for new metrics
pub fn record_end_to_end_latency(stage: &str, duration: std::time::Duration) {
    histogram!("end_to_end_latency_seconds", duration.as_secs_f64(), "stage" => stage.to_string());
}

pub fn record_simulation_duration_ms(duration: std::time::Duration) {
    histogram!("simulation_duration_ms", duration.as_millis() as f64);
}

pub fn record_rpc_call_duration_ms(method: &str, duration: std::time::Duration) {
    // Sample to reduce exporter load for high-frequency RPC timing metrics
    // Temporarily disabled histogram to prevent exporter hangs
    // const SAMPLE_RATE: u64 = 128;
    // let c = RPC_CALL_MS_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // if c % SAMPLE_RATE == 0 {
    //     histogram!("rpc_call_duration_ms", duration.as_millis() as f64, "method" => method.to_string());
    // }
}

pub fn increment_price_cache_hits() {
    counter!("price_cache_hits_total", 1);
}

pub fn increment_price_cache_misses() {
    counter!("price_cache_misses_total", 1);
}

pub fn increment_v3_local_simulator_hits() {
    counter!("v3_local_simulator_hits_total", 1);
}

pub fn increment_v3_rpc_fallback() {
    counter!("v3_rpc_fallback_total", 1);
}

pub fn set_hot_pools_fresh_count(count: f64) {
    gauge!("hot_pools_fresh_count", count);
}

pub fn set_hot_pools_stale_count(count: f64) {
    gauge!("hot_pools_stale_count", count);
}

pub fn record_pipeline_parallelism_ratio(ratio: f64) {
    histogram!("pipeline_parallelism_ratio", ratio);
}

pub fn record_state_fetch_duration_ms(duration: std::time::Duration) {
    histogram!("state_fetch_duration_ms", duration.as_millis() as f64);
}

pub fn record_route_discovery_duration_ms(duration: std::time::Duration) {
    histogram!("route_discovery_duration_ms", duration.as_millis() as f64);
}

pub fn record_shadow_mode_deviation(pool_type: &str, deviation_bps: f64) {
    histogram!("sim_local_deviation_bps", deviation_bps, "pool_type" => pool_type.to_string());
}

/// Track local simulator hit rate
pub fn increment_local_sim_hit(pool_type: &str) {
    counter!("sim_local_hit_total", 1, "pool_type" => pool_type.to_string());
}

/// Track local simulator errors
pub fn increment_local_sim_error(pool_type: &str) {
    counter!("sim_local_error_total", 1, "pool_type" => pool_type.to_string());
}

/// Track RPC fallback usage
pub fn increment_rpc_fallback(pool_type: &str) {
    counter!("sim_rpc_fallback_total", 1, "pool_type" => pool_type.to_string());
}

/// Track shadow mode mismatches (deviation > threshold)
pub fn increment_shadow_mismatch(pool_type: &str) {
    counter!("sim_shadow_mismatch_total", 1, "pool_type" => pool_type.to_string());
}

// === Adaptive sizing helpers ===
pub fn record_sizing_selected_amount_usd(amount_usd: f64, rule: &str, clamped: bool) {
    histogram!("sizing_selected_amount_usd", amount_usd, "rule" => rule.to_string());
    counter!("sizing_rule_usage_total", 1, "rule" => rule.to_string());
    if clamped {
        counter!("sizing_clamped_total", 1);
    }
}

// Pool filter metrics
pub fn increment_pool_filter_rejected(reason: &str) {
    counter!("pool_filter_rejected_total", 1, "reason" => reason.to_string());
}

pub fn increment_pool_filter_passed() {
    increment_counter!("pool_filter_passed_total");
}

pub fn increment_pool_filter_effective_liquidity_too_low() {
    increment_counter!("pool_filter_effective_liquidity_too_low_total");
}

pub fn increment_pool_filter_price_deviation_too_high() {
    increment_counter!("pool_filter_price_deviation_too_high_total");
}

pub fn increment_pool_filter_stale_data() {
    increment_counter!("pool_filter_stale_data_total");
}

pub fn increment_pool_filter_reserve_too_small() {
    increment_counter!("pool_filter_reserve_too_small_total");
}

// FASE 0: Local V3 Simulator metrics
pub fn increment_simulator_local_calls() {
    increment_counter!("simulator_local_calls_total");
}

pub fn increment_simulator_local_fallback_rpc() {
    increment_counter!("simulator_local_fallback_rpc_total");
}

pub fn record_simulator_local_match_rate(rate: f64) {
    gauge!("simulator_local_match_rate", rate);
}

pub fn record_simulator_local_deviation_bps(deviation_bps: f64) {
    histogram!("simulator_local_deviation_bps", deviation_bps);
}

// FASE 4: Debug metrics for V3 pool refresh and filtering
pub fn increment_v3_pools_refresh_attempted(count: u64) {
    counter!("v3_pools_refresh_attempted_total", count);
}

pub fn increment_v3_pools_refresh_succeeded(count: u64) {
    counter!("v3_pools_refresh_succeeded_total", count);
}

pub fn increment_v3_pools_refresh_failed(reason: &str) {
    counter!("v3_pools_refresh_failed_total", 1, "reason" => reason.to_string());
}

pub fn increment_v3_pools_filtered_by_stale(count: u64) {
    counter!("v3_pools_filtered_by_stale_total", count);
}

pub fn increment_v3_pools_filtered_by_dead(count: u64) {
    counter!("v3_pools_filtered_by_dead_total", count);
}

pub fn increment_v3_pools_filtered_by_invalid_fee(count: u64) {
    counter!("v3_pools_filtered_by_invalid_fee_total", count);
}

pub fn set_v3_pools_state_quality_fresh(count: f64) {
    gauge!("v3_pools_state_quality_fresh", count);
}

pub fn set_v3_pools_state_quality_stale(count: f64) {
    gauge!("v3_pools_state_quality_stale", count);
}

// ✅ FASE 10: Streaming Discovery Metrics
pub fn increment_streaming_discovery_blocks_received() {
    increment_counter!("streaming_discovery_blocks_received_total");
}

pub fn increment_streaming_discovery_pool_candidates(priority: &str) {
    counter!("streaming_discovery_pool_candidates_total", 1, "priority" => priority.to_string());
}

pub fn increment_streaming_discovery_pools_validated(result: &str) {
    counter!("streaming_discovery_pools_validated_total", 1, "result" => result.to_string());
}

pub fn increment_streaming_discovery_pools_inserted(count: u64) {
    counter!("streaming_discovery_pools_inserted_total", count);
}

pub fn increment_streaming_discovery_pools_updated(count: u64) {
    counter!("streaming_discovery_pools_updated_total", count);
}

pub fn set_streaming_discovery_deferred_queue_size(size: f64) {
    gauge!("streaming_discovery_deferred_queue_size", size);
}

pub fn increment_streaming_discovery_deferred_pools_processed(priority: &str, count: u64) {
    counter!("streaming_discovery_deferred_pools_processed_total", count, "priority" => priority.to_string());
}

pub fn record_streaming_discovery_event_extraction(duration: std::time::Duration) {
    histogram!(
        "streaming_discovery_event_extraction_ms",
        duration.as_millis() as f64
    );
}

pub fn record_streaming_discovery_validation(duration: std::time::Duration) {
    histogram!(
        "streaming_discovery_validation_ms",
        duration.as_millis() as f64
    );
}

pub fn record_streaming_discovery_processing(duration: std::time::Duration) {
    histogram!(
        "streaming_discovery_processing_ms",
        duration.as_millis() as f64
    );
}

// ✅ FASE 10: BlockStream Metrics
pub fn increment_blockstream_blocks_published() {
    increment_counter!("blockstream_blocks_published_total");
}

pub fn set_blockstream_active_subscribers(count: f64) {
    gauge!("blockstream_active_subscribers", count);
}

pub fn increment_blockstream_lag_events(skipped_blocks: u64) {
    counter!("blockstream_lag_events_total", skipped_blocks);
}

// ✅ FASE 10: Combined Multicall Metrics
pub fn increment_combined_multicall_executions() {
    increment_counter!("combined_multicall_executions_total");
}

pub fn record_combined_multicall_discovery_calls(count: f64) {
    histogram!("combined_multicall_discovery_calls", count);
}

pub fn record_combined_multicall_total_calls(count: f64) {
    histogram!("combined_multicall_total_calls", count);
}

pub fn increment_combined_multicall_split_events() {
    increment_counter!("combined_multicall_split_events_total");
}

// --- FASE 3: Wrapper Discovery Metrics ---
pub fn record_wrapper_discovery_duration(duration: std::time::Duration) {
    histogram!(
        "bot_wrapper_discovery_duration_seconds",
        duration.as_secs_f64()
    );
}

pub fn increment_wrapper_cache_hit() {
    increment_counter!("bot_wrapper_cache_hits_total");
}

pub fn increment_wrapper_cache_miss() {
    increment_counter!("bot_wrapper_cache_misses_total");
}

// --- FASE 3: Redis Metrics ---
pub fn increment_redis_operation(operation: &str) {
    counter!("redis_operations_total", 1, "operation" => operation.to_string());
}

pub fn record_redis_operation_duration(operation: &str, duration: std::time::Duration) {
    histogram!("redis_operation_duration_seconds", duration.as_secs_f64(), "operation" => operation.to_string());
}

pub fn increment_redis_connection_error() {
    increment_counter!("redis_connection_errors_total");
}

pub fn increment_redis_cache_hit() {
    increment_counter!("redis_cache_hits_total");
}

pub fn increment_redis_cache_miss() {
    increment_counter!("redis_cache_misses_total");
}

// --- FASE 3: Database Metrics ---
pub fn increment_db_operation(operation: &str) {
    counter!("bot_db_operations_total", 1, "operation" => operation.to_string());
}

pub fn record_db_operation_duration(operation: &str, duration: std::time::Duration) {
    histogram!("bot_db_operation_duration_seconds", duration.as_secs_f64(), "operation" => operation.to_string());
}

pub fn increment_db_connection_error() {
    increment_counter!("bot_db_connection_errors_total");
}

pub fn set_db_connections_active(count: f64) {
    gauge!("bot_db_connections_active", count);
}

pub fn set_db_connections_idle(count: f64) {
    gauge!("bot_db_connections_idle", count);
}

// --- OPTIMIZATION: RPC Calls Metrics ---
pub fn increment_rpc_call(component: &str) {
    counter!("rpc_calls_total", 1, "component" => component.to_string());
}

// --- FASE 1.2: BlockParser get_logs Metrics ---
/// Incrementa el contador de get_logs del BlockParser cuando touched.len() < 3
pub fn increment_block_parser_get_logs(
    reason: &str,
    touched_count: usize,
    pool_addresses_count: usize,
    block_number: u64,
) {
    counter!(
        "block_parser_get_logs_total",
        1,
        "reason" => reason.to_string(),
        "touched_count" => touched_count.to_string(),
        "pool_addresses_count" => pool_addresses_count.to_string(),
        "block_number" => block_number.to_string(),
    );
}

pub fn set_rpc_calls_per_block(component: &str, count: f64) {
    gauge!("rpc_calls_per_block", count, "component" => component.to_string());
}

pub fn record_rpc_call_latency(component: &str, call_type: &str, duration: std::time::Duration) {
    histogram!("rpc_call_latency_ms", duration.as_millis() as f64,
               "component" => component.to_string(),
               "call_type" => call_type.to_string());
}

// --- RPC Tracing Metrics (Fase 1.1) ---
pub fn increment_rpc_call_by_method(component: &str, method: &str) {
    counter!("rpc_calls_by_method_total", 1,
             "component" => component.to_string(),
             "method" => method.to_string());
}

pub fn record_rpc_cu_cost(component: &str, method: &str, cu_cost: f64) {
    // Prometheus counters solo aceptan u64, convertir f64 a u64 (multiplicar por 100 para mantener precisión)
    let cu_cost_u64 = (cu_cost * 100.0) as u64;
    counter!("rpc_cu_consumption_total", cu_cost_u64,
             "component" => component.to_string(),
             "method" => method.to_string());
}

pub fn record_rpc_payload_size(component: &str, method: &str, size_bytes: usize) {
    histogram!("rpc_payload_size_bytes", size_bytes as f64,
               "component" => component.to_string(),
               "method" => method.to_string());
}

// --- OPTIMIZATION: Data Coherence Metrics ---
pub fn set_data_coherence_active_pools_count(count: f64) {
    gauge!("data_coherence_active_pools_count", count);
}

pub fn set_data_coherence_stale_state_count(count: f64) {
    gauge!("data_coherence_stale_state_count", count);
}

pub fn set_data_coherence_pools_without_weights(count: f64) {
    gauge!("data_coherence_pools_without_weights", count);
}

pub fn set_data_coherence_stale_weights_count(count: f64) {
    gauge!("data_coherence_stale_weights_count", count);
}

pub fn increment_data_coherence_check(component: &str, status: &str) {
    counter!("data_coherence_check_total", 1,
             "component" => component.to_string(),
             "status" => status.to_string());
}

// --- OPTIMIZATION: Cache Hit Rate Metrics ---
pub fn set_background_validator_cache_hit_rate(rate: f64) {
    gauge!("background_validator_cache_hit_rate", rate);
}

pub fn set_background_validator_cache_size(size: f64) {
    gauge!("background_validator_cache_size", size);
}

// --- MVP Metrics ---
pub fn increment_mvp_routes_filtered(reason: &str) {
    increment_counter!("mvp_routes_filtered_total", "reason" => reason.to_string());
}

pub fn increment_mvp_reserve_validation_aborts() {
    increment_counter!("mvp_reserve_validation_aborts_total");
}

pub fn increment_mvp_reserve_validation_success() {
    increment_counter!("mvp_reserve_validation_success_total");
}

pub fn increment_mvp_auto_pairs_built() {
    increment_counter!("mvp_auto_pairs_built_total");
}

pub fn set_mvp_auto_pairs_size(size: usize) {
    gauge!("mvp_auto_pairs_size", size as f64);
}
