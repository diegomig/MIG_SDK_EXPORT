use config::{Config, ConfigError, File};
use serde::Deserialize;
use serde_json;
use std::collections::HashMap;
use std::env;

#[derive(Debug, Deserialize, Clone)]
pub struct HealthCheck {
    pub interval_seconds: u64,
    /// 🚀 RPC OPTIMIZATION: Enable lazy health checks (only check on error, not periodically)
    #[serde(default = "default_lazy_health_checks")]
    pub lazy_mode: bool,
}

fn default_lazy_health_checks() -> bool {
    true // Por defecto lazy (solo verificar cuando hay error)
}

#[derive(Debug, Deserialize, Clone)]
pub struct PriceFeeds {
    pub chainlink_oracles: HashMap<String, String>,
    #[serde(default)]
    pub feed_registry_address: Option<String>,
    #[serde(default = "default_price_feed_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,
    #[serde(default = "default_false")]
    pub enable_twap_fallback: bool,
    #[serde(default = "default_price_deviation_tolerance_bps")]
    pub price_deviation_tolerance_bps: u32,
}

fn default_false() -> bool {
    false
}
fn default_true() -> bool {
    true
}
// Note: default_true() is also used later in the file via #[serde(default = "default_true")]
fn default_price_deviation_tolerance_bps() -> u32 {
    200
} // 2%

impl Default for PriceFeeds {
    fn default() -> Self {
        Self {
            chainlink_oracles: HashMap::new(),
            feed_registry_address: None,
            cache_ttl_seconds: default_price_feed_cache_ttl_seconds(),
            enable_twap_fallback: default_false(),
            price_deviation_tolerance_bps: default_price_deviation_tolerance_bps(),
        }
    }
}

fn default_price_feed_cache_ttl_seconds() -> u64 {
    10 // 10 seconds
}

#[derive(Debug, Deserialize, Clone)]
pub struct CircuitBreaker {
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
}

fn default_failure_threshold() -> u32 {
    5
}
fn default_cooldown_seconds() -> u64 {
    60
}
fn default_provider_backoff_base_ms() -> u64 {
    200
}
fn default_provider_backoff_max_ms() -> u64 {
    5000
}
fn default_max_concurrent_per_provider() -> usize {
    8
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            cooldown_seconds: default_cooldown_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RpcProviderRole {
    General,
    Discovery,
    State,
    Submission,
}

impl Default for RpcProviderRole {
    fn default() -> Self {
        RpcProviderRole::General
    }
}

fn default_provider_roles() -> Vec<RpcProviderRole> {
    vec![
        RpcProviderRole::General,
        RpcProviderRole::Discovery,
        RpcProviderRole::State,
        RpcProviderRole::Submission,
    ]
}

#[derive(Debug, Deserialize, Clone)]
pub struct RpcProviderConfig {
    pub url: String,
    #[serde(default = "default_provider_roles")]
    pub roles: Vec<RpcProviderRole>,
    #[serde(default)]
    pub qps_limit: Option<u32>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    #[serde(default)]
    pub multicall_batch_size: Option<usize>,
    #[serde(default)]
    pub multicall_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub multicall_max_retries: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightNode {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_light_node_url")]
    pub url: String,
    #[serde(default = "default_health_check_interval_seconds")]
    pub health_check_interval_seconds: u64,
    #[serde(default = "default_false")]
    pub pre_warm_connections: bool,
    #[serde(default = "default_pre_warm_count")]
    pub pre_warm_count: usize,
    // ✅ P0 OPTIMIZATION: Auto-detect local nodes
    #[serde(default = "default_true")]
    pub auto_detect: bool,
    #[serde(default = "default_local_node_ports")]
    pub local_node_ports: Vec<u16>,
    #[serde(default = "default_arbitrum_chain_id")]
    pub expected_chain_id: u64,
}

fn default_light_node_url() -> String {
    "http://127.0.0.1:8545".to_string()
}
fn default_health_check_interval_seconds() -> u64 {
    60 // 🚀 RPC OPTIMIZATION: Increased from 30s to 60s to reduce RPC calls
}
fn default_pre_warm_count() -> usize {
    20
}

// Note: default_true() is defined at line 35, removed duplicate here

fn default_local_node_ports() -> Vec<u16> {
    vec![8545, 8546, 9545] // Standard ports: Geth HTTP, Geth WS, Reth HTTP
}

fn default_arbitrum_chain_id() -> u64 {
    42161 // Arbitrum One chain ID
}

impl Default for LightNode {
    fn default() -> Self {
        Self {
            enabled: false,
            url: default_light_node_url(),
            health_check_interval_seconds: default_health_check_interval_seconds(),
            pre_warm_connections: false,
            pre_warm_count: default_pre_warm_count(),
            auto_detect: default_true(),
            local_node_ports: default_local_node_ports(),
            expected_chain_id: default_arbitrum_chain_id(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RpcTracing {
    #[serde(default = "default_rpc_tracing_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rpc_tracing_log_level")]
    pub log_level: String, // "debug", "info", "warn"
}

impl Default for RpcTracing {
    fn default() -> Self {
        Self {
            enabled: default_rpc_tracing_enabled(),
            log_level: default_rpc_tracing_log_level(),
        }
    }
}

fn default_rpc_tracing_enabled() -> bool {
    false // Por defecto deshabilitado para no agregar latencia
}

fn default_rpc_tracing_log_level() -> String {
    "warn".to_string() // Solo loggear warnings (get_logs) por defecto
}

#[derive(Debug, Deserialize, Clone)]
pub struct Rpc {
    #[serde(default)]
    pub http_urls: Vec<String>,
    #[serde(default)]
    pub ws_urls: Vec<String>,
    #[serde(default)]
    pub providers: Vec<RpcProviderConfig>,
    pub health_check: HealthCheck,
    #[serde(default)]
    pub circuit_breaker: CircuitBreaker,
    #[serde(default = "default_provider_backoff_base_ms")]
    pub provider_backoff_base_ms: u64,
    #[serde(default = "default_provider_backoff_max_ms")]
    pub provider_backoff_max_ms: u64,
    #[serde(default = "default_max_concurrent_per_provider")]
    pub max_concurrent_per_provider: usize,
    #[serde(default)]
    pub light_node: LightNode,
    #[serde(default)]
    pub tracing: RpcTracing,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Factories {
    pub multicall: String,
    pub sushiswap_v2: String,
    pub camelot_v2: String,
    pub camelot_v3: String,
    pub pancakeswap_v2: String,
    pub kyberswap_elastic: String,
    pub traderjoe_v2: String,
    pub uniswap_v2: String,
    pub uniswap_v3: String,
    pub balancer_vault: String,
    pub curve_address_provider: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Tokens {
    pub weth: ethers::types::Address,
}

impl Default for Tokens {
    fn default() -> Self {
        // Default to zero address for tests; production should override via Config.toml/env
        Self {
            weth: ethers::types::Address::zero(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Contracts {
    pub multi_arbitrage_address: String,
    pub factories: Factories,
    #[serde(default)]
    pub tokens: Tokens,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ActivityRules {
    #[serde(default = "default_min_v2_reserve")]
    pub min_v2_reserve_usd: f64,
    #[serde(default = "default_min_v3_liquidity")]
    pub min_v3_liquidity_usd: f64,
    #[serde(default = "default_max_missing_state_hits")]
    pub max_missing_state_hits: u8,
}

fn default_min_v2_reserve() -> f64 {
    1000.0
}
fn default_min_v3_liquidity() -> f64 {
    1000.0
}
fn default_max_missing_state_hits() -> u8 {
    3
}

impl Default for ActivityRules {
    fn default() -> Self {
        Self {
            min_v2_reserve_usd: default_min_v2_reserve(),
            min_v3_liquidity_usd: default_min_v3_liquidity(),
            max_missing_state_hits: default_max_missing_state_hits(),
        }
    }
}

/// Unified liquidity thresholds for topology mapping (single source of truth)
#[derive(Debug, Deserialize, Clone)]
pub struct LiquidityThresholds {
    #[serde(default = "default_min_discovery_usd")]
    pub min_discovery_usd: f64,
    #[serde(default = "default_min_hot_cache_usd")]
    pub min_hot_cache_usd: f64,
    #[serde(default = "default_min_simple_route_usd")]
    pub min_simple_route_usd: f64,
    #[serde(default = "default_min_triangular_route_usd")]
    pub min_triangular_route_usd: f64,
    #[serde(default = "default_min_multihop_per_hop_usd")]
    pub min_multihop_per_hop_usd: f64,
    #[serde(default = "default_cleanup_enabled")]
    pub cleanup_enabled: bool,
    #[serde(default = "default_cleanup_cycles_threshold")]
    pub cleanup_cycles_threshold: u32,
}

/// OPTION 8: Configuration for continuous route pre-computation
/*#[derive(Debug, Deserialize, Clone)]
pub struct RoutePrecompute {
    #[serde(default = "default_route_precompute_min_liquidity_usd")]
    pub min_liquidity_usd: f64,
    #[serde(default = "default_route_precompute_max_liquidity_usd")]
    pub max_liquidity_usd: f64,
    #[serde(default = "default_top_n_routes")]
    pub top_n_routes: usize,
    #[serde(default = "default_continuous")]
    pub continuous: bool,
    #[serde(default = "default_validate_cached_routes")]
    pub validate_cached_routes: bool,
    #[serde(default = "default_route_precompute_max_price_deviation_bps")]
    pub max_price_deviation_bps: u32,
    #[serde(default = "default_fallback_to_dynamic")]
    pub fallback_to_dynamic: bool,
}

fn default_route_precompute_min_liquidity_usd() -> f64 {
    50_000.0
}
fn default_route_precompute_max_liquidity_usd() -> f64 {
    10_000_000.0
}
fn default_top_n_routes() -> usize {
    1000
}
fn default_continuous() -> bool {
    true
}
fn default_validate_cached_routes() -> bool {
    true
}
fn default_route_precompute_max_price_deviation_bps() -> u32 {
    500 // 5%
}
fn default_fallback_to_dynamic() -> bool {
    true
}

impl Default for RoutePrecompute {
    fn default() -> Self {
        Self {
            min_liquidity_usd: default_route_precompute_min_liquidity_usd(),
            max_liquidity_usd: default_route_precompute_max_liquidity_usd(),
            top_n_routes: default_top_n_routes(),
            continuous: default_continuous(),
            validate_cached_routes: default_validate_cached_routes(),
            max_price_deviation_bps: default_route_precompute_max_price_deviation_bps(),
            fallback_to_dynamic: default_fallback_to_dynamic(),
        }
    }
}
*/

fn default_min_discovery_usd() -> f64 {
    1000.0
}
fn default_min_hot_cache_usd() -> f64 {
    10000.0
}
fn default_min_simple_route_usd() -> f64 {
    10000.0
}
fn default_min_triangular_route_usd() -> f64 {
    50000.0
}
fn default_min_multihop_per_hop_usd() -> f64 {
    10000.0
}
fn default_cleanup_enabled() -> bool {
    true
}
fn default_cleanup_cycles_threshold() -> u32 {
    10
}

impl Default for LiquidityThresholds {
    fn default() -> Self {
        Self {
            min_discovery_usd: default_min_discovery_usd(),
            min_hot_cache_usd: default_min_hot_cache_usd(),
            min_simple_route_usd: default_min_simple_route_usd(),
            min_triangular_route_usd: default_min_triangular_route_usd(),
            min_multihop_per_hop_usd: default_min_multihop_per_hop_usd(),
            cleanup_enabled: default_cleanup_enabled(),
            cleanup_cycles_threshold: default_cleanup_cycles_threshold(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Validator {
    pub anchor_tokens: Vec<String>,
    pub blacklisted_tokens: Vec<String>,
    #[serde(default)]
    pub whitelisted_factories: Vec<String>,
    pub whitelisted_bytecode_hashes: HashMap<String, String>,
    #[serde(default)]
    pub require_anchor_token: bool,
    #[serde(default)]
    pub activity_rules: ActivityRules,
    #[serde(default)]
    pub normalization_rules: NormalizationRules,
    #[serde(default)]
    pub protocol_addresses: ProtocolAddressesSettings,
}

fn default_min_normalization_liquidity_usd() -> f64 {
    1_500.0
}

fn default_max_normalization_decimal_diff() -> u8 {
    12
}

fn default_max_normalization_token_decimals() -> u8 {
    24
}

#[derive(Debug, Deserialize, Clone)]
pub struct NormalizationRules {
    #[serde(default = "default_min_normalization_liquidity_usd")]
    pub min_liquidity_usd: f64,
    #[serde(default = "default_max_normalization_decimal_diff")]
    pub max_decimal_diff: u8,
    #[serde(default = "default_max_normalization_token_decimals")]
    pub max_token_decimals: u8,
    #[serde(default)]
    pub transfer_tax_tokens: Vec<String>,
}

impl Default for NormalizationRules {
    fn default() -> Self {
        Self {
            min_liquidity_usd: default_min_normalization_liquidity_usd(),
            max_decimal_diff: default_max_normalization_decimal_diff(),
            max_token_decimals: default_max_normalization_token_decimals(),
            transfer_tax_tokens: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AdaptiveSampling {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hot_threshold")]
    pub hot_threshold: f64,
    #[serde(default = "default_warm_interval")]
    pub warm_interval_blocks: u64,
    #[serde(default = "default_cold_interval")]
    pub cold_interval_blocks: u64,
    #[serde(default = "default_adaptive_query_interval_seconds")]
    pub adaptive_query_interval_seconds: u64,
    #[serde(default = "default_adaptive_query_frequency_seconds")]
    pub adaptive_query_frequency_seconds: u64,
}
fn default_hot_threshold() -> f64 {
    0.8
}
fn default_warm_interval() -> u64 {
    10
}
fn default_cold_interval() -> u64 {
    50
}
fn default_adaptive_query_interval_seconds() -> u64 {
    1
}
fn default_adaptive_query_frequency_seconds() -> u64 {
    10
}

impl Default for AdaptiveSampling {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            hot_threshold: default_hot_threshold(),
            warm_interval_blocks: default_warm_interval(),
            cold_interval_blocks: default_cold_interval(),
            adaptive_query_interval_seconds: default_adaptive_query_interval_seconds(),
            adaptive_query_frequency_seconds: default_adaptive_query_frequency_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProtocolFingerprint {
    #[serde(default)]
    pub dex_label: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub factories: Vec<String>,
    #[serde(default)]
    pub bytecode_hashes: Vec<String>,
    #[serde(default)]
    pub init_code_hashes: Vec<String>,
}

fn default_pending_base_delay_ms() -> u64 {
    5_000
}
fn default_pending_max_delay_ms() -> u64 {
    120_000
}
fn default_pending_max_attempts() -> u32 {
    6
}
fn default_pending_max_batch() -> usize {
    64
}

#[derive(Debug, Deserialize, Clone)]
pub struct PendingQueueSettings {
    #[serde(default = "default_pending_base_delay_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_pending_max_delay_ms")]
    pub max_delay_ms: u64,
    #[serde(default = "default_pending_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_pending_max_batch")]
    pub max_batch: usize,
}

impl Default for PendingQueueSettings {
    fn default() -> Self {
        Self {
            base_delay_ms: default_pending_base_delay_ms(),
            max_delay_ms: default_pending_max_delay_ms(),
            max_attempts: default_pending_max_attempts(),
            max_batch: default_pending_max_batch(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProtocolAddressesSettings {
    #[serde(default)]
    pub balancer_vault: Option<String>,
    #[serde(default)]
    pub curve_address_provider: Option<String>,
}

fn default_scheduler_base_weight() -> f64 {
    1.0
}
fn default_scheduler_min_weight() -> f64 {
    0.2
}
fn default_scheduler_latency_smoothing() -> f64 {
    0.2
}
fn default_scheduler_latency_normalizer_ms() -> f64 {
    3000.0
}
fn default_scheduler_failure_penalty() -> f64 {
    0.75
}
fn default_scheduler_recovery_factor() -> f64 {
    0.5
}

#[derive(Debug, Deserialize, Clone)]
pub struct AdaptiveSchedulerSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_scheduler_base_weight")]
    pub base_weight: f64,
    #[serde(default = "default_scheduler_min_weight")]
    pub min_weight: f64,
    #[serde(default = "default_scheduler_latency_smoothing")]
    pub latency_smoothing: f64,
    #[serde(default = "default_scheduler_latency_normalizer_ms")]
    pub latency_normalizer_ms: f64,
    #[serde(default = "default_scheduler_failure_penalty")]
    pub failure_penalty: f64,
    #[serde(default = "default_scheduler_recovery_factor")]
    pub recovery_factor: f64,
}

impl Default for AdaptiveSchedulerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            base_weight: default_scheduler_base_weight(),
            min_weight: default_scheduler_min_weight(),
            latency_smoothing: default_scheduler_latency_smoothing(),
            latency_normalizer_ms: default_scheduler_latency_normalizer_ms(),
            failure_penalty: default_scheduler_failure_penalty(),
            recovery_factor: default_scheduler_recovery_factor(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Discovery {
    pub initial_sync_blocks: u64,
    #[serde(default = "default_discovery_interval")]
    pub interval_seconds: u64,
    #[serde(default)]
    pub adaptive_sampling: AdaptiveSampling,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_fetch_token_symbols")]
    pub fetch_token_symbols: bool,
    #[serde(default)]
    pub protocol_fingerprints: HashMap<String, ProtocolFingerprint>,
    #[serde(default)]
    pub pending_queue: PendingQueueSettings,
    #[serde(default)]
    pub adaptive_scheduler: AdaptiveSchedulerSettings,

    // Phase 2: Forward scan for recent pools
    #[serde(default = "default_enable_forward_scan")]
    pub enable_forward_scan: bool,

    #[serde(default = "default_forward_scan_blocks")]
    pub forward_scan_blocks: u64,
}

fn default_fetch_token_symbols() -> bool {
    true // Enable symbol fetching by default
}

fn default_enable_forward_scan() -> bool {
    true // Enable forward scan by default (Phase 2)
}

fn default_forward_scan_blocks() -> u64 {
    100 // Scan last 100 blocks (~5 minutes on Arbitrum)
}

fn default_db_path() -> String {
    "postgresql://user:pass@127.0.0.1:5432/mig_topology".to_string()
}

fn default_discovery_interval() -> u64 {
    300
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackgroundDiscoverer {
    #[serde(default = "default_fast_lane_interval")]
    pub fast_lane_interval_seconds: u64,
}

fn default_fast_lane_interval() -> u64 {
    300 // 5 minutos (optimizado desde 60s)
}

impl Default for BackgroundDiscoverer {
    fn default() -> Self {
        Self {
            fast_lane_interval_seconds: default_fast_lane_interval(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackgroundValidator {
    #[serde(default = "default_min_interval")]
    pub min_interval_seconds: u64,
    #[serde(default = "default_max_interval")]
    pub max_interval_seconds: u64,
    #[serde(default = "default_target_cache_hit_rate")]
    pub target_cache_hit_rate: f64,
}

fn default_min_interval() -> u64 {
    60
}
fn default_max_interval() -> u64 {
    120
}
fn default_target_cache_hit_rate() -> f64 {
    0.90
}

impl Default for BackgroundValidator {
    fn default() -> Self {
        Self {
            min_interval_seconds: default_min_interval(),
            max_interval_seconds: default_max_interval(),
            target_cache_hit_rate: default_target_cache_hit_rate(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct GraphConfig {
    #[serde(default = "default_graph_update_interval_seconds")]
    pub update_interval_seconds: u64,
}

fn default_graph_update_interval_seconds() -> u64 {
    60 // Default: update graph weights every 60 seconds
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            update_interval_seconds: default_graph_update_interval_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct JitFetcher {
    #[serde(default = "default_false")]
    pub fetch_balances: bool,
    #[serde(default = "default_false")]
    pub validate_consistency: bool,
}

impl Default for JitFetcher {
    fn default() -> Self {
        Self {
            fetch_balances: false, // Optimizado: no fetchear balances por defecto
            validate_consistency: false,
        }
    }
}

fn default_max_daily_volume_usd() -> f64 {
    500_000.0
}
fn default_min_liquidity_multiplier() -> f64 {
    1.5
}
fn default_stop_loss_percentage() -> f64 {
    0.8
}

fn default_pause_duration() -> u64 {
    3600 // 1 hour
}

fn default_uncertainty_bps_per_hop() -> u32 {
    1 // 0.01%
}

fn default_max_fee_per_gas() -> f64 {
    20.0
}
fn default_max_priority_fee_per_gas() -> f64 {
    1.5
}

fn default_simulation_slippage_bps() -> u32 {
    5 // 0.05%
}

fn default_safety_bps_per_hop() -> u32 {
    2
}

#[derive(Debug, Deserialize, Clone)]
pub struct Performance {
    #[serde(default = "default_precision_check_top_n")]
    pub precision_check_top_n: usize,
    pub get_logs_chunk_size: u64,
    pub get_logs_max_concurrency: usize,
    pub global_qps_limit: u32,
    pub multicall_batch_size: usize,
    #[serde(default = "default_multicall_timeout_seconds")]
    pub multicall_timeout_seconds: u64,
    #[serde(default = "default_multicall_max_retries")]
    pub multicall_max_retries: u32,
    pub max_concurrent_requests_per_host: usize,
    #[serde(default = "default_simulation_concurrency")]
    pub simulation_concurrency: usize,
    #[serde(default = "default_max_route_hops")]
    pub max_route_hops: usize,
    #[serde(default = "default_route_search_branching_factor")]
    pub route_search_branching_factor: usize,
    #[serde(default = "default_state_refresh_top_k")]
    pub state_refresh_top_k: usize,
    #[serde(default = "default_shadow_estimate_top_n")]
    pub shadow_estimate_top_n: usize,
    #[serde(default = "default_hot_pool_refresh_interval_ms")]
    pub hot_pool_refresh_interval_ms: u64,
    #[serde(default = "default_cold_pool_refresh_interval_ms")]
    pub cold_pool_refresh_interval_ms: u64,
    #[serde(default = "default_route_upper_bound_filter")]
    pub route_upper_bound_filter: bool,
    // ✅ SPRINT ESTABILIZACIÓN: Configuración de cache JIT
    #[serde(default = "default_jit_cache_tolerance_blocks")]
    pub jit_cache_tolerance_blocks: u64,
    #[serde(default = "default_jit_cache_ttl_ms")]
    pub jit_cache_ttl_ms: u64,
    // ✅ P0 OPTIMIZATION: TTL diferenciado para touched/untouched pools
    #[serde(default = "default_touched_pool_ttl_seconds")]
    pub touched_pool_ttl_seconds: u64,
    #[serde(default = "default_untouched_pool_ttl_seconds")]
    pub untouched_pool_ttl_seconds: u64,
    #[serde(default = "default_adaptive_ttl_enabled")]
    pub adaptive_ttl_enabled: bool,
    #[serde(default = "default_adaptive_ttl_weight_threshold")]
    pub adaptive_ttl_weight_threshold: f64,
    // ✅ P1 OPTIMIZATION: Parallel price fetching
    #[serde(default = "default_parallel_price_fetching_enabled")]
    pub parallel_price_fetching_enabled: bool,
    #[serde(default = "default_price_fetch_chunk_size")]
    pub price_fetch_chunk_size: usize,
    #[serde(default = "default_min_trade_size_usd")]
    pub min_trade_size_usd: f64,
    #[serde(default = "default_max_trade_liquidity_pct")]
    pub max_trade_liquidity_pct: f64,
    #[serde(default = "default_max_price_impact_pct")]
    pub max_price_impact_pct: f64,
    #[serde(default = "default_hot_detector_max_hot_pools")]
    pub hot_detector_max_hot_pools: usize,
    #[serde(default = "default_hot_detector_max_parallel_groups")]
    pub hot_detector_max_parallel_groups: usize,
    #[serde(default = "default_hot_detector_max_pools_per_group")]
    pub hot_detector_max_pools_per_group: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolFilters {
    #[serde(default = "default_min_effective_liquidity_eth")]
    pub min_effective_liquidity_eth: f64,
    #[serde(default = "default_max_price_deviation_bps")]
    pub max_price_deviation_bps: u32,
    #[serde(default = "default_max_stale_blocks")]
    pub max_stale_blocks: u64,
    #[serde(default = "default_min_volume_24h_usd")]
    pub min_volume_24h_usd: f64,
    #[serde(default = "default_allowed_fee_tiers")]
    pub allowed_fee_tiers: Vec<u32>,
    #[serde(default = "default_allowed_dexs")]
    pub allowed_dexs: Vec<String>,
    #[serde(default = "default_min_reserve_multiplier")]
    pub min_reserve_multiplier: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SimulatorConfig {
    #[serde(default = "default_enable_local_v3")]
    pub enable_local_v3: bool,
    #[serde(default = "default_allow_rpc_fallback")]
    pub allow_rpc_fallback: bool,
    #[serde(default = "default_shadow_mode_enabled")]
    pub shadow_mode_enabled: bool,
    #[serde(default = "default_shadow_max_deviation_bps")]
    pub shadow_max_deviation_bps: u32,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            enable_local_v3: default_enable_local_v3(),
            allow_rpc_fallback: default_allow_rpc_fallback(),
            shadow_mode_enabled: default_shadow_mode_enabled(),
            shadow_max_deviation_bps: default_shadow_max_deviation_bps(),
        }
    }
}

fn default_enable_local_v3() -> bool {
    true
}

fn default_allow_rpc_fallback() -> bool {
    true
}

fn default_shadow_mode_enabled() -> bool {
    false // Disabled by default (dev only)
}

fn default_shadow_max_deviation_bps() -> u32 {
    10 // 0.1% max deviation before alerting
}

fn default_min_effective_liquidity_eth() -> f64 {
    3.0
}

fn default_max_price_deviation_bps() -> u32 {
    30
}

fn default_max_stale_blocks() -> u64 {
    3
}

fn default_min_volume_24h_usd() -> f64 {
    50000.0
}

fn default_allowed_fee_tiers() -> Vec<u32> {
    vec![5, 30, 100]
}

fn default_allowed_dexs() -> Vec<String> {
    vec![
        "Uniswap V3".to_string(),
        "Sushiswap".to_string(),
        "Camelot".to_string(),
        "Kyber Elastic".to_string(),
        "Ramses V2".to_string(),
        "Ramses V3".to_string(),
        "Trader Joe".to_string(),
        "Balancer".to_string(),
    ]
}

fn default_min_reserve_multiplier() -> f64 {
    5.0
}

impl Default for PoolFilters {
    fn default() -> Self {
        Self {
            min_effective_liquidity_eth: default_min_effective_liquidity_eth(),
            max_price_deviation_bps: default_max_price_deviation_bps(),
            max_stale_blocks: default_max_stale_blocks(),
            min_volume_24h_usd: default_min_volume_24h_usd(),
            allowed_fee_tiers: default_allowed_fee_tiers(),
            allowed_dexs: default_allowed_dexs(),
            min_reserve_multiplier: default_min_reserve_multiplier(),
        }
    }
}

fn default_max_route_hops() -> usize {
    3
}

fn default_max_trade_size_usd() -> f64 {
    20000.0
}

fn default_min_trade_size_usd() -> f64 {
    10.0
}

// ✅ SPRINT ESTABILIZACIÓN: Defaults para cache JIT
fn default_jit_cache_tolerance_blocks() -> u64 {
    5 // Pools no-touched válidos por hasta 5 bloques
}

fn default_jit_cache_ttl_ms() -> u64 {
    30000 // 30 segundos TTL para pools no-touched
}

fn default_touched_pool_ttl_seconds() -> u64 {
    30 // 30 seconds for touched pools
}

fn default_untouched_pool_ttl_seconds() -> u64 {
    300 // 5 minutes for untouched pools
}

fn default_adaptive_ttl_enabled() -> bool {
    true
}

fn default_adaptive_ttl_weight_threshold() -> f64 {
    100_000.0 // $100K USD threshold
}

fn default_parallel_price_fetching_enabled() -> bool {
    true
}

fn default_price_fetch_chunk_size() -> usize {
    20 // 20 tokens per chunk for parallel fetching
}

fn default_max_trade_liquidity_pct() -> f64 {
    20.0
}

fn default_max_price_impact_pct() -> f64 {
    30.0
}

fn default_multicall_timeout_seconds() -> u64 {
    20
}

fn default_multicall_max_retries() -> u32 {
    3
}

fn default_route_search_branching_factor() -> usize {
    5
}

fn default_simulation_concurrency() -> usize {
    4
}

fn default_state_refresh_top_k() -> usize {
    1000
}

fn default_shadow_estimate_top_n() -> usize {
    3
}
fn default_hot_pool_refresh_interval_ms() -> u64 {
    12000
}
fn default_cold_pool_refresh_interval_ms() -> u64 {
    60000
}
fn default_route_upper_bound_filter() -> bool {
    true
}
fn default_min_profit_threshold_usd() -> f64 {
    5.0
}
fn default_precision_check_top_n() -> usize {
    3
}
fn default_hot_detector_max_hot_pools() -> usize {
    500
}
fn default_hot_detector_max_parallel_groups() -> usize {
    4
}
fn default_hot_detector_max_pools_per_group() -> usize {
    150
}

#[derive(Debug, Deserialize, Clone)]
pub struct Metrics {
    pub enabled: bool,
    pub port: u16,
}

// === Data Quality Settings ===
#[derive(Debug, Deserialize, Clone)]
pub struct DataQuality {
    #[serde(default = "default_state_max_age_secs")]
    pub state_max_age_secs: u64,
    #[serde(default = "default_price_deviation_pct_threshold")]
    pub price_deviation_pct_threshold: f64,
    #[serde(default = "default_min_liquidity_usd")]
    pub min_liquidity_usd: f64,
    #[serde(default = "default_approximate_max_hops")]
    pub approximate_max_hops: usize,
    #[serde(default = "default_max_ratio_per_route")]
    pub max_ratio_per_route: f64,
}

fn default_state_max_age_secs() -> u64 {
    120
}
fn default_price_deviation_pct_threshold() -> f64 {
    2.0
}
fn default_min_liquidity_usd() -> f64 {
    1000.0
}
fn default_approximate_max_hops() -> usize {
    1
}
fn default_max_ratio_per_route() -> f64 {
    2.0
}

impl Default for DataQuality {
    fn default() -> Self {
        Self {
            state_max_age_secs: default_state_max_age_secs(),
            price_deviation_pct_threshold: default_price_deviation_pct_threshold(),
            min_liquidity_usd: default_min_liquidity_usd(),
            approximate_max_hops: default_approximate_max_hops(),
            max_ratio_per_route: default_max_ratio_per_route(),
        }
    }
}

/// ✅ FASE 3.3: Feature flags for all fallbacks and optimizations
#[derive(Debug, Deserialize, Clone)]
pub struct Features {
    pub enable_execution: bool,
    #[serde(default)]
    pub enabled_dexes: HashMap<String, Vec<String>>,
    pub log_json_report: bool,
    /// ✅ FASE 3.3: Enable WebSocket block number subscription
    #[serde(default = "default_true")]
    pub enable_websocket_blocks: bool,
    /// ✅ FASE 3.3: Enable polling fallback for block numbers (if WebSocket fails)
    #[serde(default = "default_true")]
    pub enable_polling_fallback: bool,
    /// ✅ FASE 3.3: Enable event indexing for gap detection
    #[serde(default = "default_true")]
    pub enable_event_indexing: bool,
    /// ✅ FASE 3.3: Enable price feed fallback chain (Chainlink → CoinGecko → TWAP)
    #[serde(default = "default_true")]
    pub enable_price_fallback_chain: bool,
    /// ✅ FASE 3.3: Enable Merkle tree cache for JIT state fetcher
    #[serde(default = "default_true")]
    pub enable_merkle_cache: bool,
    /// ✅ FASE 3.3: Enable streaming multicall
    #[serde(default = "default_true")]
    pub enable_streaming_multicall: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub enum LogFormat {
    #[serde(rename = "json")]
    Json,
    #[default]
    #[serde(rename = "pretty")]
    Pretty,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogSettings {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub format: LogFormat,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Chain {
    pub chain_id: u64,
}

fn default_log_level() -> String {
    "info".to_string()
}

/*#[derive(Debug, Deserialize, Clone)]
pub struct SignerSettings {
    #[serde(default = "default_signer_enabled")]
    pub enabled: bool,
    pub base_url: String,
    pub ca_cert_path: String,
    pub client_cert_path: String,
    pub client_key_path: String,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub auth_token_path: Option<String>,
    #[serde(default = "default_signer_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_signer_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_signer_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_signer_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default = "default_signer_rate_limit_per_second")]
    pub rate_limit_per_second: u32,
    #[serde(default = "default_signer_rate_limit_burst")]
    pub rate_limit_burst: u32,
    pub account_address: String,
}

fn default_signer_enabled() -> bool {
    false // Default to disabled (dry-run mode)
}

fn default_signer_request_timeout_ms() -> u64 {
    1500
}

fn default_signer_connect_timeout_ms() -> u64 {
    500
}

fn default_signer_max_retries() -> u32 {
    3
}

fn default_signer_retry_backoff_ms() -> u64 {
    200
}

fn default_signer_rate_limit_per_second() -> u32 {
    20
}

fn default_signer_rate_limit_burst() -> u32 {
    5
}
*/

#[derive(Debug, Deserialize, Clone)]
pub struct Fees {
    #[serde(default)]
    pub v2_fees_bps: HashMap<String, u32>,
}

impl Default for Fees {
    fn default() -> Self {
        let mut v2_fees_bps = HashMap::new();
        v2_fees_bps.insert("UniswapV2".to_string(), 30);
        v2_fees_bps.insert("SushiSwapV2".to_string(), 30);
        v2_fees_bps.insert("PancakeSwap".to_string(), 25);
        v2_fees_bps.insert("TraderJoe".to_string(), 30);
        v2_fees_bps.insert("CamelotV2".to_string(), 30);
        Self { v2_fees_bps }
    }
}

/*#[derive(Debug, Deserialize, Clone)]
pub struct Strategy {
    #[serde(default = "default_true")]
    pub enable_dynamic_sizing: bool,
    #[serde(default = "default_max_price_impact_bps")]
    pub max_price_impact_bps: u32,
    #[serde(default = "default_true")]
    pub enable_adaptive_slippage: bool,
    #[serde(default = "default_min_slippage_bps")]
    pub min_slippage_bps: u32,
    #[serde(default = "default_max_slippage_bps")]
    pub max_slippage_bps: u32,
}

fn default_max_price_impact_bps() -> u32 {
    50
} // 0.5%
fn default_min_slippage_bps() -> u32 {
    10
} // 0.1%
fn default_max_slippage_bps() -> u32 {
    50
} // 0.5%

impl Default for Strategy {
    fn default() -> Self {
        Self {
            enable_dynamic_sizing: true,
            max_price_impact_bps: default_max_price_impact_bps(),
            enable_adaptive_slippage: true,
            min_slippage_bps: default_min_slippage_bps(),
            max_slippage_bps: default_max_slippage_bps(),
        }
    }
}
*/

#[derive(Debug, Deserialize, Clone)]
pub struct Sizing {
    #[serde(default = "default_v2_fraction_divisor")]
    pub v2_fraction_divisor: u32,
    #[serde(default = "default_v3_liquidity_fraction")]
    pub v3_liquidity_fraction: f64,
    #[serde(default = "default_curve_liquidity_fraction")]
    pub curve_liquidity_fraction: f64,
    #[serde(default = "default_balancer_liquidity_fraction")]
    pub balancer_liquidity_fraction: f64,
    #[serde(default = "default_min_usd")]
    pub min_usd: f64,
    #[serde(default = "default_max_usd")]
    pub max_usd: f64,
    #[serde(default = "default_target_impact_bps")]
    pub target_impact_bps: u32,
    #[serde(default = "default_enable_liquidity_cap")]
    pub enable_liquidity_cap: bool,
    #[serde(default = "default_liquidity_cap_fraction")]
    pub liquidity_cap_fraction: f64,
    #[serde(default)]
    pub token_caps_usd: HashMap<String, f64>,
    #[serde(default)]
    pub enable_ufr_micro_sampling: bool,
    #[serde(default = "default_enable_optimal_search")]
    pub enable_optimal_search: bool,
    #[serde(default = "default_max_search_latency_ms")]
    pub max_search_latency_ms: u64,
    #[serde(default = "default_use_heuristic_if_hops_gt")]
    pub use_heuristic_if_hops_gt: usize,
    #[serde(default = "default_max_search_points")]
    pub max_search_points: usize,
}

fn default_v2_fraction_divisor() -> u32 {
    200
}
fn default_v3_liquidity_fraction() -> f64 {
    0.01
}
fn default_curve_liquidity_fraction() -> f64 {
    0.001
}
fn default_balancer_liquidity_fraction() -> f64 {
    0.005
}
fn default_min_usd() -> f64 {
    500.0
}
fn default_max_usd() -> f64 {
    500_000.0
}
fn default_target_impact_bps() -> u32 {
    75
}
fn default_enable_liquidity_cap() -> bool {
    true
}
fn default_liquidity_cap_fraction() -> f64 {
    0.04
}
fn default_enable_optimal_search() -> bool {
    true
}
fn default_max_search_latency_ms() -> u64 {
    5
}
fn default_use_heuristic_if_hops_gt() -> usize {
    3
}
fn default_max_search_points() -> usize {
    15
}

fn default_warming_enabled() -> bool {
    true
}

fn default_warming_candidate_limit() -> usize {
    3000
}

fn default_warming_hot_pool_target() -> usize {
    500
}

fn default_warming_min_liquidity_usd() -> f64 {
    1000.0
}

fn default_warming_max_liquidity_usd() -> f64 {
    1_000_000_000.0
}

fn default_warming_cross_check_bps() -> f64 {
    200.0
}

fn default_warming_minimum_viable_pools() -> usize {
    300
}

fn default_warming_dex_slots() -> Vec<WarmingDexSlot> {
    vec![
        WarmingDexSlot {
            dex: "UniswapV3".to_string(),
            share: 0.4,
            floor: 50,
        },
        WarmingDexSlot {
            dex: "UniswapV2".to_string(),
            share: 0.25,
            floor: 40,
        },
        WarmingDexSlot {
            dex: "SushiSwapV2".to_string(),
            share: 0.12,
            floor: 20,
        },
        WarmingDexSlot {
            dex: "Balancer".to_string(),
            share: 0.08,
            floor: 10,
        },
        WarmingDexSlot {
            dex: "Curve".to_string(),
            share: 0.08,
            floor: 10,
        },
        WarmingDexSlot {
            dex: "KyberSwap".to_string(),
            share: 0.07,
            floor: 10,
        },
    ]
}

#[derive(Debug, Deserialize, Clone)]
pub struct Warming {
    #[serde(default = "default_warming_enabled")]
    pub enabled: bool,
    #[serde(default = "default_warming_candidate_limit")]
    pub candidate_limit: usize,
    #[serde(default = "default_warming_hot_pool_target")]
    pub hot_pool_target: usize,
    #[serde(default = "default_warming_min_liquidity_usd")]
    pub min_liquidity_usd: f64,
    #[serde(default = "default_warming_max_liquidity_usd")]
    pub max_liquidity_usd: f64,
    #[serde(default = "default_warming_cross_check_bps")]
    pub cross_check_bps: f64,
    #[serde(default = "default_warming_minimum_viable_pools")]
    pub minimum_viable_pools: usize,
    #[serde(default = "default_warming_dex_slots")]
    pub dex_slots: Vec<WarmingDexSlot>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WarmingDexSlot {
    pub dex: String,
    pub share: f64,
    #[serde(default)]
    pub floor: usize,
}

impl Default for Warming {
    fn default() -> Self {
        Self {
            enabled: default_warming_enabled(),
            candidate_limit: default_warming_candidate_limit(),
            hot_pool_target: default_warming_hot_pool_target(),
            min_liquidity_usd: default_warming_min_liquidity_usd(),
            max_liquidity_usd: default_warming_max_liquidity_usd(),
            cross_check_bps: default_warming_cross_check_bps(),
            minimum_viable_pools: default_warming_minimum_viable_pools(),
            dex_slots: default_warming_dex_slots(),
        }
    }
}

impl Default for Sizing {
    fn default() -> Self {
        Self {
            v2_fraction_divisor: default_v2_fraction_divisor(),
            v3_liquidity_fraction: default_v3_liquidity_fraction(),
            curve_liquidity_fraction: default_curve_liquidity_fraction(),
            balancer_liquidity_fraction: default_balancer_liquidity_fraction(),
            min_usd: default_min_usd(),
            max_usd: default_max_usd(),
            target_impact_bps: default_target_impact_bps(),
            enable_liquidity_cap: default_enable_liquidity_cap(),
            liquidity_cap_fraction: default_liquidity_cap_fraction(),
            token_caps_usd: HashMap::new(),
            enable_ufr_micro_sampling: false,
            enable_optimal_search: default_enable_optimal_search(),
            max_search_latency_ms: default_max_search_latency_ms(),
            use_heuristic_if_hops_gt: default_use_heuristic_if_hops_gt(),
            max_search_points: default_max_search_points(),
        }
    }
}

// MVP: Minimal Viable Product restrictions (corset mode)
#[derive(Debug, Deserialize, Clone)]
pub struct MVP {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default)]
    pub auto: MVPAuto,
    #[serde(default)]
    pub dex_whitelist: Vec<String>,
    #[serde(default)]
    pub token_whitelist: Vec<ethers::types::Address>,
    #[serde(default)]
    pub pair_whitelist: Vec<String>, // e.g., ["WETH-USDC", "USDC-USDT"]
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    #[serde(default = "default_max_routes_per_cycle")]
    pub max_routes_per_cycle: usize,
    #[serde(default = "default_true")]
    pub revalidate_reserves_before_execution: bool,
    #[serde(default = "default_max_reserve_change_bps")]
    pub max_reserve_change_bps: u32,
    #[serde(default = "default_max_reverts_per_day")]
    pub max_reverts_per_day: u32,
}

fn default_max_hops() -> usize {
    2
}
fn default_max_routes_per_cycle() -> usize {
    1
}
fn default_max_reserve_change_bps() -> u32 {
    50 // 0.5%
}
fn default_max_reverts_per_day() -> u32 {
    3
}

// MVP Auto-config for dynamic pair selection
#[derive(Debug, Deserialize, Clone, Default)]
pub struct MVPAuto {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_min_tvl_usd")]
    pub min_tvl_usd: f64,
    #[serde(default = "default_max_tvl_usd")]
    pub max_tvl_usd: f64,
    #[serde(default = "default_skip_top_tvl_pools")]
    pub skip_top_tvl_pools: u32,
    #[serde(default)]
    pub exclude_tokens: Vec<ethers::types::Address>,
    #[serde(default)]
    pub min_pool_age_blocks: Option<u64>,
    #[serde(default)]
    pub max_pool_age_days: Option<u64>,
    #[serde(default)]
    pub max_price_impact_bps: Option<u32>,
    #[serde(default)]
    pub max_pairs: Option<usize>,
    #[serde(default)]
    pub min_profit_to_gas_ratio: Option<f64>,
    #[serde(default)]
    pub max_exec_per_day: Option<u32>,
}

fn default_min_tvl_usd() -> f64 {
    5_000.0
}
fn default_max_tvl_usd() -> f64 {
    50_000.0
}
fn default_skip_top_tvl_pools() -> u32 {
    20
}

impl Default for MVP {
    fn default() -> Self {
        Self {
            enabled: false,
            auto: MVPAuto {
                enabled: false,
                min_tvl_usd: default_min_tvl_usd(),
                max_tvl_usd: default_max_tvl_usd(),
                skip_top_tvl_pools: default_skip_top_tvl_pools(),
                exclude_tokens: Vec::new(),
                min_pool_age_blocks: None,
                max_pool_age_days: None,
                max_price_impact_bps: None,
                max_pairs: None,
                min_profit_to_gas_ratio: None,
                max_exec_per_day: None,
            },
            dex_whitelist: Vec::new(),
            token_whitelist: Vec::new(),
            pair_whitelist: Vec::new(),
            max_hops: default_max_hops(),
            max_routes_per_cycle: default_max_routes_per_cycle(),
            revalidate_reserves_before_execution: default_true(),
            max_reserve_change_bps: default_max_reserve_change_bps(),
            max_reverts_per_day: default_max_reverts_per_day(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub rpc: Rpc,
    pub contracts: Contracts,
    pub validator: Validator,
    pub discovery: Discovery,
    #[serde(default)]
    pub price_feeds: PriceFeeds,
    pub performance: Performance,
    pub metrics: Metrics,
    pub features: Features,
    #[serde(default)]
    pub liquidity: LiquidityThresholds,
    pub log: LogSettings,
    pub chain: Chain,
    #[serde(default)]
    pub fees: Fees,
    #[serde(default)]
    pub data_quality: DataQuality,
    #[serde(default)]
    pub sizing: Sizing,
    #[serde(default)]
    pub warming: Warming,
    #[serde(default)]
    pub pool_filters: PoolFilters,
    #[serde(default)]
    pub simulator: SimulatorConfig,
    #[serde(default)]
    pub background_discoverer: BackgroundDiscoverer,
    #[serde(default)]
    pub background_validator: BackgroundValidator,
    #[serde(default)]
    pub jit_fetcher: JitFetcher,
    #[serde(default)]
    pub graph: GraphConfig,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .add_source(File::with_name("Config.toml"))
            .build()?;

        let mut settings: Self = s.try_deserialize()?;

        // Environment variable overrides for RPC configuration
        if let Ok(raw_http) = env::var("SDK_RPC_HTTP_URLS") {
            if let Some(list) = parse_string_list(&raw_http) {
                if !list.is_empty() {
                    settings.rpc.http_urls = list;
                }
            }
        }
        if let Ok(raw_ws) = env::var("SDK_RPC_WS_URLS") {
            if let Some(list) = parse_string_list(&raw_ws) {
                if !list.is_empty() {
                    settings.rpc.ws_urls = list;
                }
            }
        }

        // Optional: Chainlink oracles mapping override via ENV (JSON: { token_address: oracle_address })
        if let Ok(raw_oracles) = env::var("SDK_PRICE_FEEDS_CHAINLINK_ORACLES") {
            let trimmed = raw_oracles.trim();
            if !trimmed.is_empty() {
                match serde_json::from_str::<std::collections::HashMap<String, String>>(trimmed) {
                    Ok(map) => {
                        for (token, oracle) in map {
                            // only insert non-empty strings; address validation is done later in context
                            if !token.trim().is_empty() && !oracle.trim().is_empty() {
                                settings.price_feeds.chainlink_oracles.insert(token, oracle);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to parse SDK_PRICE_FEEDS_CHAINLINK_ORACLES as JSON: {}",
                            e
                        );
                    }
                }
            }
        }

        // Optional: contracts.tokens.weth override via ENV
        if let Ok(weth_env) = env::var("SDK_CONTRACTS_TOKENS_WETH") {
            let trimmed = weth_env.trim();
            if !trimmed.is_empty() {
                if let Ok(addr) = trimmed.parse() {
                    settings.contracts.tokens.weth = addr;
                }
            }
        }

        ensure_rpc_providers(&mut settings);

        Ok(settings)
    }
}

fn parse_string_list(input: &str) -> Option<Vec<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(vec![]);
    }

    // Si parece JSON (empieza con '['), intentar parsear como JSON
    if trimmed.starts_with('[') {
        match serde_json::from_str::<Vec<String>>(trimmed) {
            Ok(v) => return Some(v),
            Err(_) => {
                // Fallback: Remover corchetes y parsear manualmente
                let without_brackets = trimmed.trim_start_matches('[').trim_end_matches(']').trim();

                // Si después de quitar corchetes tiene comillas, parsear como JSON string
                if without_brackets.starts_with('"') || without_brackets.starts_with('\'') {
                    let parts: Vec<String> = without_brackets
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    return Some(parts);
                }

                // Si no tiene comillas pero tiene comas, separar por comas (URLs sin comillas)
                if without_brackets.contains(',') {
                    let parts: Vec<String> = without_brackets
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    return Some(parts);
                }

                // Si no tiene comillas ni comas, es una URL directa
                if !without_brackets.is_empty() {
                    return Some(vec![without_brackets.to_string()]);
                }
            }
        }
    }

    // Fallback final: separar por coma
    let parts: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(parts)
}

fn ensure_rpc_providers(settings: &mut Settings) {
    if settings.rpc.providers.is_empty() && !settings.rpc.http_urls.is_empty() {
        settings.rpc.providers = settings
            .rpc
            .http_urls
            .iter()
            .map(|url| RpcProviderConfig {
                url: url.clone(),
                roles: default_provider_roles(),
                qps_limit: None,
                max_concurrency: None,
                multicall_batch_size: None,
                multicall_timeout_seconds: None,
                multicall_max_retries: None,
            })
            .collect();
    }
}
