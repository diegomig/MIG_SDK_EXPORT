// src/rpc_pool.rs

use crate::flight_recorder::FlightRecorder;
use crate::metrics;
use crate::rpc_tracing_middleware::estimate_cu_cost;
use crate::settings::Settings;
use crate::{record_decision, record_phase_end, record_phase_start, record_rpc_call};
use anyhow::Result;
use ethers::middleware::Middleware;
use ethers::prelude::{Http, Provider};
use ethers::types::Address;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use log::{debug, info, warn};
use rand::Rng;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::pin;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::time::sleep;

type DefaultDirectRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// Enum interno para manejar providers (simplificado - tracing directo en métodos)
#[derive(Clone)]
enum ProviderWrapper {
    Direct(Arc<Provider<Http>>),
}

impl ProviderWrapper {
    /// Extrae el Provider<Http> interno del wrapper
    fn as_provider(&self) -> Arc<Provider<Http>> {
        match self {
            Self::Direct(p) => Arc::clone(p),
        }
    }

    /// Llama a get_block_number en el provider correcto con tracing
    async fn get_block_number(
        &self,
        flight_recorder: Option<Arc<FlightRecorder>>,
        endpoint: &str,
    ) -> Result<ethers::types::U64, ethers::providers::ProviderError> {
        match self {
            Self::Direct(p) => {
                // Tracing directo (más simple y pragmático)
                let start = std::time::Instant::now();
                let method = "eth_blockNumber";
                let result = p.get_block_number().await;
                let duration = start.elapsed();
                let success = result.is_ok();

                // Registrar métricas
                let component = "rpc_pool";
                let cu_cost = estimate_cu_cost(method, 0);
                metrics::increment_rpc_call(component);
                metrics::increment_rpc_call_by_method(component, method);
                metrics::record_rpc_cu_cost(component, method, cu_cost);
                metrics::record_rpc_payload_size(component, method, 0);
                metrics::record_rpc_call_latency(component, method, duration);

                // ✅ FLIGHT RECORDER: Registrar evento RPC
                if let Some(ref recorder) = flight_recorder {
                    record_rpc_call!(recorder, endpoint, method, start, success);
                }

                log::debug!(
                    "[RPC_TRACE] {} -> {}: duration={:?}, cu_cost={:.2}",
                    component,
                    method,
                    duration,
                    cu_cost
                );
                result
            }
        }
    }

    /// Llama a get_logs en el provider correcto con tracing
    async fn get_logs(
        &self,
        filter: &ethers::types::Filter,
        flight_recorder: Option<Arc<FlightRecorder>>,
        endpoint: &str,
    ) -> Result<Vec<ethers::types::Log>, ethers::providers::ProviderError> {
        match self {
            Self::Direct(p) => {
                // Tracing directo
                let start = std::time::Instant::now();
                let method = "eth_getLogs";

                // Estimar payload
                let filter_debug = format!("{:?}", filter);
                let address_count = match &filter.address {
                    Some(ethers::types::ValueOrArray::Value(_)) => 1,
                    Some(ethers::types::ValueOrArray::Array(arr)) => arr.len(),
                    None => 0,
                };
                let topic_count_est = filter_debug.matches("topic").count();
                let payload_size = address_count * 20 + topic_count_est * 32;
                let cu_cost = estimate_cu_cost(method, payload_size);

                log::warn!("🚨 [RPC_TRACE] rpc_pool -> {} TRIGGERED: payload_est={} bytes, cu_estimate={:.2}",
                          method, payload_size, cu_cost);

                let result = p.get_logs(filter).await;
                let duration = start.elapsed();
                let success = result.is_ok();

                // Obtener número de logs para el evento
                // Note: Filter doesn't expose block_range directly, so we can't extract block number from it
                let logs_count = result.as_ref().ok().map(|logs| logs.len()).unwrap_or(0);

                // Registrar métricas
                let component = "rpc_pool";
                metrics::increment_rpc_call(component);
                metrics::increment_rpc_call_by_method(component, method);
                metrics::record_rpc_cu_cost(component, method, cu_cost);
                metrics::record_rpc_payload_size(component, method, payload_size);
                metrics::record_rpc_call_latency(component, method, duration);

                // ✅ FLIGHT RECORDER: Registrar evento RPC con detalles completos
                // Note: We can't extract block number from Filter, so we record without it
                if let Some(ref recorder) = flight_recorder {
                    record_rpc_call!(recorder, endpoint, method, start, success);
                }

                match &result {
                    Ok(logs) => {
                        log::warn!("🚨 [RPC_TRACE] {} -> {} SUCCESS: logs_count={}, duration={:?}, cu_cost={:.2}",
                                  component, method, logs.len(), duration, cu_cost);
                    }
                    Err(e) => {
                        log::warn!(
                            "🚨 [RPC_TRACE] {} -> {} ERROR: {:?}, duration={:?}, cu_cost={:.2}",
                            component,
                            method,
                            e,
                            duration,
                            cu_cost
                        );
                    }
                }

                result
            }
        }
    }
}

impl std::fmt::Debug for ProviderWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct(_) => write!(f, "ProviderWrapper::Direct"),
        }
    }
}

// No implementamos Middleware para ProviderWrapper porque las firmas del trait
// tienen lifetimes específicos que son difíciles de satisfacer.
// En su lugar, extraemos el provider correcto cuando es necesario.

/// Role of an RPC provider in the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpcRole {
    General,
    Discovery,
    State,
    Submission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerStateName {
    Closed,
    Open,
    HalfOpen,
}

/// Circuit breaker state for RPC provider health management.
///
/// Tracks provider health and implements automatic failover when providers become unhealthy.
#[derive(Debug, Clone)]
pub struct CircuitBreakerState {
    state: CircuitBreakerStateName,
    failures: u32,
    last_failure: Option<Instant>,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            state: CircuitBreakerStateName::Closed,
            failures: 0,
            last_failure: None,
        }
    }
}

/// Status information for an RPC provider in the pool.
///
/// Tracks provider health, latency, and usage statistics for load balancing.
#[derive(Clone)]
pub struct ProviderStatus {
    provider: Arc<ProviderWrapper>,
    url: String,
    is_healthy: bool,
    backoff_until: Arc<Mutex<Instant>>,
    limiter: Arc<DefaultDirectRateLimiter>,
    semaphore: Arc<Semaphore>,
    // Adaptive concurrency fields
    latency_tracker: Arc<Mutex<Vec<u128>>>,
    success_count: Arc<AtomicUsize>,
    failure_count: Arc<AtomicUsize>,
    current_concurrency: Arc<AtomicUsize>,
    rate_limit_errors: Arc<AtomicU8>,
    circuit_breaker: Arc<Mutex<CircuitBreakerState>>,
    // ✅ FLIGHT RECORDER: Optional recorder for RPC call events
    flight_recorder: Option<Arc<FlightRecorder>>,
}

/// Manages a pool of RPC providers with health checking to improve reliability.
#[derive(Clone)]
/// Load-balanced RPC provider pool with automatic failover.
///
/// Manages multiple RPC providers with intelligent load balancing, health monitoring,
/// and automatic failover for high availability.
///
/// ## Features
///
/// - **Load Balancing**: Distributes requests across healthy providers
/// - **Circuit Breakers**: Automatic failover for unhealthy providers
/// - **Rate Limiting**: Respects provider rate limits
/// - **Health Monitoring**: Tracks latency and success rates
///
/// ## Usage
///
/// ```rust
/// let rpc_pool = Arc::new(RpcPool::new(Arc::new(settings))?);
/// let (provider, permit) = rpc_pool.get_next_provider().await?;
/// // Use provider...
/// rpc_pool.report_success(&provider, duration);
/// ```
pub struct RpcPool {
    providers: Arc<Mutex<Vec<ProviderStatus>>>,
    current_index: Arc<AtomicUsize>,
    backoff_level: Arc<AtomicU8>,
    settings: Arc<Settings>,
    // ✅ FLIGHT RECORDER: Optional recorder for instrumentation
    flight_recorder: Option<Arc<FlightRecorder>>,
}

impl RpcPool {
    /// Creates a new RpcPool and spawns a background task for health checks.
    /// P0.3: Extended to support light node localhost as primary RPC.
    pub fn new(settings: Arc<Settings>) -> Result<Self> {
        let quota = Quota::per_second(
            NonZeroU32::new(settings.performance.global_qps_limit)
                .ok_or_else(|| anyhow::anyhow!("QPS must be non-zero"))?,
        );

        let mut providers_status: Vec<ProviderStatus> = Vec::new();

        // P0.3: Add light node as primary if enabled
        if settings.rpc.light_node.enabled {
            let light_node_url = &settings.rpc.light_node.url;
            if let Ok(base_provider) = Provider::<Http>::try_from(light_node_url.as_str()) {
                // Tracing se hace directamente en ProviderWrapper::get_block_number y get_logs
                let provider = Arc::new(ProviderWrapper::Direct(Arc::new(base_provider)));

                let initial_concurrency = settings.performance.max_concurrent_requests_per_host;
                let light_node_status = ProviderStatus {
                    provider,
                    url: light_node_url.clone(),
                    is_healthy: true,
                    backoff_until: Arc::new(Mutex::new(Instant::now())),
                    limiter: Arc::new(RateLimiter::direct(quota.clone())),
                    semaphore: Arc::new(Semaphore::new(initial_concurrency)),
                    latency_tracker: Arc::new(Mutex::new(Vec::with_capacity(100))),
                    success_count: Arc::new(AtomicUsize::new(0)),
                    failure_count: Arc::new(AtomicUsize::new(0)),
                    current_concurrency: Arc::new(AtomicUsize::new(initial_concurrency)),
                    rate_limit_errors: Arc::new(AtomicU8::new(0)),
                    circuit_breaker: Arc::new(Mutex::new(CircuitBreakerState::default())),
                    flight_recorder: None, // Will be set via with_flight_recorder()
                };
                providers_status.push(light_node_status);
                info!("P0.3: Light node added as primary RPC: {}", light_node_url);
            } else {
                warn!("P0.3: Failed to create light node provider from {}, falling back to remote RPCs", light_node_url);
            }
        }

        /// ✅ P0 OPTIMIZATION: Auto-detect local node by checking standard ports
        /// Returns the URL of the first working local node found, or None if none detected
        async fn detect_local_node(settings: &Settings) -> Option<String> {
            use tokio::time::{timeout, Duration};

            let ports = &settings.rpc.light_node.local_node_ports;
            let expected_chain_id = settings.rpc.light_node.expected_chain_id;

            for port in ports {
                let url = format!("http://127.0.0.1:{}", port);

                // Try to create provider and verify chain ID
                if let Ok(provider) = Provider::<Http>::try_from(url.as_str()) {
                    // Quick health check: verify chain ID matches Arbitrum One
                    let check_result = timeout(Duration::from_millis(500), async {
                        provider.get_chainid().await
                    })
                    .await;

                    match check_result {
                        Ok(Ok(chain_id)) => {
                            let chain_id_u64: u64 = chain_id.as_u64();
                            if chain_id_u64 == expected_chain_id {
                                info!(
                                    "✅ [P0] Detected local node at {} with correct chain ID ({})",
                                    url, chain_id_u64
                                );
                                return Some(url);
                            } else {
                                debug!(
                                    "⚠️ [P0] Local node at {} has wrong chain ID: {} (expected {})",
                                    url, chain_id_u64, expected_chain_id
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            debug!("⚠️ [P0] Failed to get chain ID from {}: {:?}", url, e);
                        }
                        Err(_) => {
                            debug!("⚠️ [P0] Timeout checking chain ID from {}", url);
                        }
                    }
                }
            }

            None
        }

        /// ✅ P0 OPTIMIZATION: Create ProviderStatus for local node with optimized settings
        /// Local nodes get:
        /// - Higher concurrency (2x normal)
        /// - More permits in semaphore
        /// - Note: Keep-alive connections are handled by the underlying HTTP client in ethers
        /// - Shorter timeout expectations (handled at application level)
        async fn create_local_provider_status(
            url: &str,
            quota: &Quota,
            settings: &Settings,
        ) -> Result<ProviderStatus> {
            // Use standard provider creation - ethers handles HTTP connection pooling internally
            let base_provider = Provider::<Http>::try_from(url)?;
            let provider = Arc::new(ProviderWrapper::Direct(Arc::new(base_provider)));

            // ✅ P0 OPTIMIZATION: Local nodes get higher concurrency (2x normal) and more permits
            let initial_concurrency = settings.performance.max_concurrent_requests_per_host * 2; // 2x for local
            let semaphore_size = initial_concurrency * 2; // Even more permits for local node (4x total)

            info!(
                "✅ [P0] Created local node provider with {}x concurrency and {} permits",
                initial_concurrency / settings.performance.max_concurrent_requests_per_host,
                semaphore_size
            );

            Ok(ProviderStatus {
                provider,
                url: url.to_string(),
                is_healthy: true,
                backoff_until: Arc::new(Mutex::new(Instant::now())),
                limiter: Arc::new(RateLimiter::direct(quota.clone())),
                semaphore: Arc::new(Semaphore::new(semaphore_size)), // More permits for local
                latency_tracker: Arc::new(Mutex::new(Vec::with_capacity(100))),
                success_count: Arc::new(AtomicUsize::new(0)),
                failure_count: Arc::new(AtomicUsize::new(0)),
                current_concurrency: Arc::new(AtomicUsize::new(initial_concurrency)),
                rate_limit_errors: Arc::new(AtomicU8::new(0)),
                circuit_breaker: Arc::new(Mutex::new(CircuitBreakerState::default())),
                flight_recorder: None, // Will be set via with_flight_recorder()
            })
        }

        // Add remote RPC providers
        let remote_providers: Vec<ProviderStatus> = settings
            .rpc
            .http_urls
            .iter()
            .filter_map(|url| {
                Provider::<Http>::try_from(url.as_str())
                    .ok()
                    .map(|p| (url.clone(), p))
            })
            .map(|(url, provider)| {
                // Tracing se hace directamente en ProviderWrapper::get_block_number y get_logs
                let provider = Arc::new(ProviderWrapper::Direct(Arc::new(provider)));

                let initial_concurrency = settings.performance.max_concurrent_requests_per_host;
                ProviderStatus {
                    provider,
                    url,
                    is_healthy: true, // Assume healthy initially
                    backoff_until: Arc::new(Mutex::new(Instant::now())),
                    limiter: Arc::new(RateLimiter::direct(quota.clone())),
                    semaphore: Arc::new(Semaphore::new(initial_concurrency)),
                    latency_tracker: Arc::new(Mutex::new(Vec::with_capacity(100))),
                    success_count: Arc::new(AtomicUsize::new(0)),
                    failure_count: Arc::new(AtomicUsize::new(0)),
                    current_concurrency: Arc::new(AtomicUsize::new(initial_concurrency)),
                    rate_limit_errors: Arc::new(AtomicU8::new(0)),
                    circuit_breaker: Arc::new(Mutex::new(CircuitBreakerState::default())),
                    flight_recorder: None, // Will be set via with_flight_recorder()
                }
            })
            .collect();

        providers_status.extend(remote_providers);

        if providers_status.is_empty() {
            return Err(anyhow::anyhow!("No valid RPC providers could be created"));
        }

        let pool = Self {
            providers: Arc::new(Mutex::new(providers_status)),
            current_index: Arc::new(AtomicUsize::new(0)),
            backoff_level: Arc::new(AtomicU8::new(0)),
            settings: settings.clone(),
            flight_recorder: None,
        };

        // P0.3: Pre-warm connections if enabled
        if settings.rpc.light_node.enabled && settings.rpc.light_node.pre_warm_connections {
            let pool_clone = pool.clone();
            let pre_warm_count = settings.rpc.light_node.pre_warm_count;
            tokio::spawn(async move {
                pool_clone.pre_warm_connections(pre_warm_count).await;
            });
        }

        // ✅ FASE 1.1: Always spawn health checker for local nodes (5s interval),
        // and general health checker if lazy_mode is disabled
        let has_local_node = {
            let guard = pool.providers.lock().unwrap();
            guard.iter().any(|s| Self::is_local_node(&s.url))
        };

        if has_local_node {
            // Spawn proactive health checker for local node (always, even in lazy mode)
            pool.spawn_health_checker();
            info!("✅ FASE 1.1: Proactive health checks enabled for local node (5s interval)");
        }

        if !settings.rpc.health_check.lazy_mode {
            pool.spawn_health_checker();
            info!(
                "🔄 Periodic health checks enabled (interval: {}s)",
                settings.rpc.health_check.interval_seconds
            );
        } else {
            info!("🚀 Lazy health checks enabled - only checking on errors (saves RPC calls)");
        }

        // P0.3: Spawn light node health checker if enabled (legacy support)
        let light_node_enabled = settings.rpc.light_node.enabled;
        if light_node_enabled {
            let pool_clone = pool.clone();
            let interval = settings.rpc.light_node.health_check_interval_seconds;
            tokio::spawn(async move {
                pool_clone.spawn_light_node_health_checker(interval).await;
            });
        }

        Ok(pool)
    }

    /// Set flight recorder for instrumentation and propagate to all ProviderStatus instances
    pub fn with_flight_recorder(mut self, recorder: Arc<FlightRecorder>) -> Self {
        self.flight_recorder = Some(recorder.clone());

        // ✅ FLIGHT RECORDER: Propagar recorder a todos los ProviderStatus
        {
            let mut providers_guard = self.providers.lock().unwrap();
            for status in providers_guard.iter_mut() {
                status.flight_recorder = Some(recorder.clone());
            }
        } // Guard dropped here, allowing self to be moved

        self
    }

    /// Gets the next healthy provider, applying rate limits and acquiring a concurrency permit.
    /// Acquire a provider for a specific role (wrapper for get_next_provider)
    pub async fn acquire(
        &self,
        role: RpcRole,
    ) -> Result<(Arc<Provider<Http>>, OwnedSemaphorePermit)> {
        let start_time = Instant::now();

        // ✅ FLIGHT RECORDER: Registrar inicio de acquire
        record_phase_start!(
            &self.flight_recorder,
            "rpc_pool_acquire",
            serde_json::json!({
                "role": format!("{:?}", role)
            })
        );

        let result = self.get_next_provider().await;

        // ✅ FLIGHT RECORDER: Registrar fin de acquire
        record_phase_end!(
            &self.flight_recorder,
            "rpc_pool_acquire",
            start_time,
            serde_json::json!({
                "success": result.is_ok(),
                "role": format!("{:?}", role)
            })
        );

        result
    }

    /// Acquire a provider with multicall setup
    pub async fn acquire_multicall(
        &self,
        role: RpcRole,
        multicall_address: Address,
        batch_size: usize,
    ) -> Result<(
        crate::multicall::Multicall<Provider<Http>>,
        (Arc<Provider<Http>>, OwnedSemaphorePermit),
    )> {
        let start_time = Instant::now();

        // ✅ FLIGHT RECORDER: Registrar inicio de acquire_multicall
        record_phase_start!(
            &self.flight_recorder,
            "rpc_pool_acquire_multicall",
            serde_json::json!({
                "role": format!("{:?}", role),
                "multicall_address": format!("{:?}", multicall_address),
                "batch_size": batch_size
            })
        );

        let result = self.get_next_provider().await;
        let (provider, permit) = match result {
            Ok((p, perm)) => (p, perm),
            Err(e) => {
                // ✅ FLIGHT RECORDER: Registrar error
                record_phase_end!(
                    &self.flight_recorder,
                    "rpc_pool_acquire_multicall",
                    start_time,
                    serde_json::json!({
                        "success": false,
                        "error": format!("{}", e)
                    })
                );
                return Err(e);
            }
        };

        let multicall =
            crate::multicall::Multicall::new(provider.clone(), multicall_address, batch_size);

        // ✅ FLIGHT RECORDER: Registrar éxito
        record_phase_end!(
            &self.flight_recorder,
            "rpc_pool_acquire_multicall",
            start_time,
            serde_json::json!({
                "success": true,
                "role": format!("{:?}", role)
            })
        );

        Ok((multicall, (provider, permit)))
    }

    pub async fn get_next_provider(&self) -> Result<(Arc<Provider<Http>>, OwnedSemaphorePermit)> {
        let start_time = Instant::now();

        self.apply_backoff_delay().await;

        let status = match self.get_next_provider_internals().await {
            Ok(s) => s,
            Err(e) => {
                // ✅ FLIGHT RECORDER: Registrar error al obtener provider
                record_decision!(
                    &self.flight_recorder,
                    "rpc_pool",
                    "provider_selection_failed",
                    &format!("{}", e),
                    serde_json::json!({
                        "error": format!("{}", e)
                    })
                );
                return Err(e);
            }
        };

        let permit = status.semaphore.clone().acquire_owned().await?;
        status.limiter.until_ready().await;

        // ✅ CRÍTICO: Crear provider FRESCO cada vez (evita EOF errors)
        // Los providers de ethers-rs mantienen estado HTTP interno que se corrompe
        // cuando se reutiliza el mismo Arc<Provider>. Crear providers frescos
        // elimina este problema completamente.
        let url = status.url.clone();
        let fresh_provider = Provider::<Http>::try_from(url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create fresh provider from {}: {}", url, e))?;
        let provider = Arc::new(fresh_provider);

        // ✅ FLIGHT RECORDER: Registrar selección exitosa de provider
        record_decision!(
            &self.flight_recorder,
            "rpc_pool",
            "provider_selected",
            "success",
            serde_json::json!({
                "url": status.url,
                "duration_ms": start_time.elapsed().as_millis()
            })
        );

        Ok((provider, permit))
    }

    /// Get provider with endpoint info for RPC recording
    /// Returns (provider, permit, endpoint_url) tuple
    pub async fn get_next_provider_with_endpoint(
        &self,
    ) -> Result<(Arc<Provider<Http>>, OwnedSemaphorePermit, String)> {
        let start_time = Instant::now();

        self.apply_backoff_delay().await;

        let status = match self.get_next_provider_internals().await {
            Ok(s) => s,
            Err(e) => {
                record_decision!(
                    &self.flight_recorder,
                    "rpc_pool",
                    "provider_selection_failed",
                    &format!("{}", e),
                    serde_json::json!({
                        "error": format!("{}", e)
                    })
                );
                return Err(e);
            }
        };

        let permit = status.semaphore.clone().acquire_owned().await?;
        status.limiter.until_ready().await;

        let url = status.url.clone();
        let endpoint = url.clone();
        let fresh_provider = Provider::<Http>::try_from(url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create fresh provider from {}: {}", url, e))?;
        let provider = Arc::new(fresh_provider);

        record_decision!(
            &self.flight_recorder,
            "rpc_pool",
            "provider_selected",
            "success",
            serde_json::json!({
                "url": endpoint,
                "duration_ms": start_time.elapsed().as_millis()
            })
        );

        Ok((provider, permit, endpoint))
    }

    /// Helper method to get block number with RPC call recording
    /// This wraps the provider.get_block_number() call and records it to Flight Recorder
    pub async fn get_block_number_with_recording(
        &self,
        provider: &Arc<Provider<Http>>,
        endpoint: &str,
    ) -> Result<ethers::types::U64, ethers::providers::ProviderError> {
        let start = std::time::Instant::now();
        let method = "eth_blockNumber";
        let result = provider.get_block_number().await;
        let duration = start.elapsed();
        let success = result.is_ok();

        // Registrar métricas
        let component = "rpc_pool";
        let cu_cost = estimate_cu_cost(method, 0);
        metrics::increment_rpc_call(component);
        metrics::increment_rpc_call_by_method(component, method);
        metrics::record_rpc_cu_cost(component, method, cu_cost);
        metrics::record_rpc_payload_size(component, method, 0);
        metrics::record_rpc_call_latency(component, method, duration);

        // ✅ FLIGHT RECORDER: Registrar evento RPC
        if let Some(ref recorder) = self.flight_recorder {
            record_rpc_call!(recorder, endpoint, method, start, success);
        }

        result
    }

    /// Helper method to get logs with RPC call recording
    /// This wraps the provider.get_logs() call and records it to Flight Recorder
    pub async fn get_logs_with_recording(
        &self,
        provider: &Arc<Provider<Http>>,
        filter: &ethers::types::Filter,
        endpoint: &str,
    ) -> Result<Vec<ethers::types::Log>, ethers::providers::ProviderError> {
        let start = std::time::Instant::now();
        let method = "eth_getLogs";

        // Estimar payload
        let filter_debug = format!("{:?}", filter);
        let address_count = match &filter.address {
            Some(ethers::types::ValueOrArray::Value(_)) => 1,
            Some(ethers::types::ValueOrArray::Array(arr)) => arr.len(),
            None => 0,
        };
        let topic_count_est = filter_debug.matches("topic").count();
        let payload_size = address_count * 20 + topic_count_est * 32;

        let result = provider.get_logs(filter).await;
        let duration = start.elapsed();
        let success = result.is_ok();
        let logs_count = result.as_ref().ok().map(|logs| logs.len()).unwrap_or(0);

        // Registrar métricas
        let component = "rpc_pool";
        let cu_cost = estimate_cu_cost(method, payload_size);
        metrics::increment_rpc_call(component);
        metrics::increment_rpc_call_by_method(component, method);
        metrics::record_rpc_cu_cost(component, method, cu_cost);
        metrics::record_rpc_payload_size(component, method, payload_size);
        metrics::record_rpc_call_latency(component, method, duration);

        // ✅ FLIGHT RECORDER: Registrar evento RPC
        if let Some(ref recorder) = self.flight_recorder {
            record_rpc_call!(recorder, endpoint, method, start, success);
        }

        result
    }

    /// Check if a URL represents a local node
    fn is_local_node(url: &str) -> bool {
        url.contains("127.0.0.1")
            || url.contains("localhost")
            || url.starts_with("http://127.0.0.1")
            || url.starts_with("http://localhost")
    }

    /// Gets the next healthy provider status object internally.
    async fn get_next_provider_internals(&self) -> Result<ProviderStatus> {
        let providers_guard = self.providers.lock().unwrap();
        let now = Instant::now();
        let mut healthy_providers: Vec<_> = providers_guard
            .iter()
            .filter(|s| {
                let backoff_until = s.backoff_until.lock().unwrap();
                let cb = s.circuit_breaker.lock().unwrap();
                s.is_healthy && now >= *backoff_until && cb.state != CircuitBreakerStateName::Open
            })
            .cloned()
            .collect();

        if healthy_providers.is_empty() {
            return Err(anyhow::anyhow!("No healthy RPC providers available"));
        }

        // ✅ P0 OPTIMIZATION: Prioritize local node first with improved sorting
        // Local nodes get highest priority, then by latency (if available), then by provider type
        healthy_providers.sort_by(|a, b| {
            let a_is_local = Self::is_local_node(&a.url);
            let b_is_local = Self::is_local_node(&b.url);
            let a_is_alchemy = a.url.contains("alchemy");
            let b_is_alchemy = b.url.contains("alchemy");
            let a_is_infura = a.url.contains("infura");
            let b_is_infura = b.url.contains("infura");

            // ✅ P0 OPTIMIZATION: Local node ALWAYS first (even if temporarily unhealthy)
            match (a_is_local, b_is_local) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }

            // ✅ P0 OPTIMIZATION: Compare by average latency if available (local nodes should be fastest)
            let a_latency = {
                let latencies = a.latency_tracker.lock().unwrap();
                if latencies.is_empty() {
                    None
                } else {
                    let total_nanos: u128 = latencies.iter().sum();
                    let total = Duration::from_nanos(total_nanos.min(u64::MAX as u128) as u64);
                    Some(total / latencies.len() as u32)
                }
            };
            let b_latency = {
                let latencies = b.latency_tracker.lock().unwrap();
                if latencies.is_empty() {
                    None
                } else {
                    let total_nanos: u128 = latencies.iter().sum();
                    let total = Duration::from_nanos(total_nanos.min(u64::MAX as u128) as u64);
                    Some(total / latencies.len() as u32)
                }
            };

            // If both have latency data, prefer faster one
            if let (Some(a_lat), Some(b_lat)) = (a_latency, b_latency) {
                match a_lat.cmp(&b_lat) {
                    std::cmp::Ordering::Less => return std::cmp::Ordering::Less,
                    std::cmp::Ordering::Greater => return std::cmp::Ordering::Greater,
                    _ => {}
                }
            }

            // Then Alchemy
            match (a_is_alchemy, b_is_alchemy) {
                (true, false) => std::cmp::Ordering::Less, // Alchemy first
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    // If neither or both are Alchemy, prioritize non-Infura
                    match (a_is_infura, b_is_infura) {
                        (true, false) => std::cmp::Ordering::Greater, // Infura last
                        (false, true) => std::cmp::Ordering::Less,
                        _ => std::cmp::Ordering::Equal,
                    }
                }
            }
        });

        // Use first provider (local node if available, then Alchemy)
        Ok(healthy_providers[0].clone())
    }

    /// Increases the backoff level, up to a maximum.
    pub fn increase_backoff(&self) {
        self.backoff_level
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                Some(std::cmp::min(v + 1, 5))
            })
            .ok(); // Cap at 2^5 * 100ms = 3.2s
    }

    /// Resets the backoff level to 0.
    pub fn reset_backoff(&self) {
        if self.backoff_level.load(Ordering::SeqCst) > 0 {
            info!("Resetting RPC backoff level to 0.");
            self.backoff_level.store(0, Ordering::SeqCst);
        }
    }

    /// Applies a global delay if the backoff level is active.
    async fn apply_backoff_delay(&self) {
        let level = self.backoff_level.load(Ordering::SeqCst);
        if level > 0 {
            let base_delay_ms = (1 << level) * 100; // e.g., 200ms, 400ms, 800ms...
            let jitter_ms = rand::thread_rng().gen_range(0..50);
            let total_delay = Duration::from_millis(base_delay_ms + jitter_ms);
            warn!(
                "Backoff active (level {}). Delaying for {:?}.",
                level, total_delay
            );
            sleep(total_delay).await;
        }
    }

    /// P0.3: Pre-warm connections to light node
    async fn pre_warm_connections(&self, count: usize) {
        let light_node_url_opt = {
            let guard = self.providers.lock().unwrap();
            guard.first().map(|s| s.url.clone())
        };

        if let Some(url) = light_node_url_opt {
            if url.contains("127.0.0.1") || url.contains("localhost") {
                info!(
                    "P0.3: Pre-warming {} connections to light node: {}",
                    count, url
                );
                let mut tasks = Vec::new();

                for i in 0..count {
                    let pool_clone = self.clone();
                    tasks.push(tokio::spawn(async move {
                        match pool_clone.get_next_provider().await {
                            Ok((provider, _permit)) => {
                                // Make a test call to warm the connection
                                let _ = provider.get_block_number().await;
                                if i % 5 == 0 {
                                    info!("P0.3: Pre-warmed connection {}/{}", i + 1, count);
                                }
                            }
                            Err(e) => {
                                warn!("P0.3: Failed to pre-warm connection {}: {}", i, e);
                            }
                        }
                    }));
                }

                // Wait for all pre-warm tasks
                futures::future::join_all(tasks).await;
                info!("P0.3: Pre-warming complete: {} connections ready", count);
            }
        }
    }

    /// P0.3: Spawn light node health checker (more frequent than general health check)
    async fn spawn_light_node_health_checker(&self, interval_seconds: u64) {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
        loop {
            interval.tick().await;

            let light_node_status_opt = {
                let guard = self.providers.lock().unwrap();
                guard.first().cloned()
            };

            if let Some(status) = light_node_status_opt {
                if status.url.contains("127.0.0.1") || status.url.contains("localhost") {
                    let start = Instant::now();
                    let recorder = status.flight_recorder.clone();
                    let endpoint = status.url.clone();
                    match status.provider.get_block_number(recorder, &endpoint).await {
                        Ok(_) => {
                            let latency = start.elapsed();
                            #[cfg(feature = "observability")]
                            metrics::histogram!("rpc_latency_ms", latency.as_millis() as f64, "provider" => "localhost");
                            if latency.as_millis() > 10 {
                                warn!(
                                    "P0.3: Light node latency higher than expected: {:?}",
                                    latency
                                );
                            }
                        }
                        Err(e) => {
                            warn!("P0.3: Light node health check failed: {}", e);
                            #[cfg(feature = "observability")]
                            metrics::counter!("rpc_health_check_failures_total", 1, "provider" => "localhost");

                            // Mark as unhealthy
                            let mut guard = self.providers.lock().unwrap();
                            if let Some(s) = guard.iter_mut().find(|s| s.url == status.url) {
                                s.is_healthy = false;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Spawns a background task to periodically check the health of providers.
    /// ✅ FASE 1.1: Health checks proactivos cada 5s para nodo local, intervalo normal para remotos
    fn spawn_health_checker(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(
                    self_clone.settings.rpc.health_check.interval_seconds,
                ))
                .await;
                info!("Running RPC provider health checks...");

                let providers_to_check: Vec<ProviderStatus> = {
                    let guard = self_clone.providers.lock().unwrap();
                    guard.iter().cloned().collect()
                };

                for status_clone in &providers_to_check {
                    status_clone.adjust_concurrency(&self_clone.settings).await;
                    let recorder = status_clone.flight_recorder.clone();
                    let endpoint = status_clone.url.clone();
                    let health_check_result = status_clone
                        .provider
                        .get_block_number(recorder, &endpoint)
                        .await;
                    let is_healthy_now = health_check_result.is_ok();

                    let mut guard = self_clone.providers.lock().unwrap();
                    if let Some(status) = guard.iter_mut().find(|s| {
                        Arc::ptr_eq(
                            &s.provider.as_provider(),
                            &status_clone.provider.as_provider(),
                        )
                    }) {
                        let mut cb = status.circuit_breaker.lock().unwrap();

                        // Handle circuit breaker state transitions
                        match cb.state {
                            CircuitBreakerStateName::Open => {
                                if let Some(last_failure) = cb.last_failure {
                                    if last_failure.elapsed().as_secs()
                                        > self_clone.settings.rpc.circuit_breaker.cooldown_seconds
                                    {
                                        cb.state = CircuitBreakerStateName::HalfOpen;
                                        cb.failures = 0;
                                        info!("Provider {} circuit breaker transitioning to HalfOpen.", status.url);
                                        metrics::set_circuit_breaker_state(&status.url, 2.0);
                                    }
                                }
                            }
                            CircuitBreakerStateName::HalfOpen => {
                                if is_healthy_now {
                                    cb.state = CircuitBreakerStateName::Closed;
                                    cb.failures = 0;
                                    info!("Provider {} circuit breaker is now Closed.", status.url);
                                    metrics::set_circuit_breaker_state(&status.url, 0.0);
                                } else {
                                    cb.state = CircuitBreakerStateName::Open;
                                    cb.last_failure = Some(Instant::now());
                                    warn!("Provider {} failed health check in HalfOpen state. Circuit breaker is now Open.", status.url);
                                    metrics::set_circuit_breaker_state(&status.url, 1.0);
                                    metrics::increment_circuit_breaker_opened(&status.url);
                                }
                            }
                            CircuitBreakerStateName::Closed => {
                                // This is handled by report_failure
                            }
                        }

                        if status.is_healthy && !is_healthy_now {
                            warn!("RPC provider failed health check: {}. Error: {}. Marking as unhealthy.", status.url, health_check_result.err().unwrap());
                            status.is_healthy = false;
                        } else if !status.is_healthy && is_healthy_now {
                            info!("RPC provider is back online: {}", status.url);
                            status.is_healthy = true;
                            status.rate_limit_errors.store(0, Ordering::SeqCst);
                            cb.state = CircuitBreakerStateName::Closed;
                            cb.failures = 0;
                            metrics::set_circuit_breaker_state(&status.url, 0.0);
                        }
                    }
                }
            }
        });

        // ✅ FASE 1.1: Spawn separate health checker for local node with 5s interval
        let self_clone_local = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;

                let local_provider_opt = {
                    let guard = self_clone_local.providers.lock().unwrap();
                    guard.iter().find(|s| Self::is_local_node(&s.url)).cloned()
                };

                if let Some(local_status) = local_provider_opt {
                    let start = Instant::now();
                    let recorder = local_status.flight_recorder.clone();
                    let endpoint = local_status.url.clone();
                    match local_status
                        .provider
                        .get_block_number(recorder, &endpoint)
                        .await
                    {
                        Ok(_) => {
                            let latency = start.elapsed();
                            #[cfg(feature = "observability")]
                            metrics::histogram!("rpc_latency_ms", latency.as_millis() as f64, "provider" => "localhost");

                            if latency.as_millis() > 10 {
                                warn!(
                                    "P1.1: Local node latency higher than expected: {:?}",
                                    latency
                                );
                            }

                            // Update health status
                            let mut guard = self_clone_local.providers.lock().unwrap();
                            if let Some(status) =
                                guard.iter_mut().find(|s| s.url == local_status.url)
                            {
                                if !status.is_healthy {
                                    info!("P1.1: Local node is back online");
                                    status.is_healthy = true;
                                    let mut cb = status.circuit_breaker.lock().unwrap();
                                    cb.state = CircuitBreakerStateName::Closed;
                                    cb.failures = 0;
                                    metrics::set_circuit_breaker_state(&status.url, 0.0);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("P1.1: Local node health check failed: {}", e);
                            #[cfg(feature = "observability")]
                            metrics::counter!("rpc_health_check_failures_total", 1, "provider" => "localhost");

                            // Mark as unhealthy
                            let mut guard = self_clone_local.providers.lock().unwrap();
                            if let Some(status) =
                                guard.iter_mut().find(|s| s.url == local_status.url)
                            {
                                if status.is_healthy {
                                    warn!("P1.1: Local node failed health check, marking as unhealthy");
                                    status.is_healthy = false;
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    /// Returns the total number of providers in the pool, regardless of health.
    pub fn provider_count(&self) -> usize {
        self.providers.lock().unwrap().len()
    }

    /// Get the fastest available provider based on recent latency (ULTRA FAST)
    pub async fn get_fastest_provider(
        &self,
    ) -> Result<(Arc<Provider<Http>>, OwnedSemaphorePermit)> {
        let providers = self.providers.lock().unwrap();

        // Find provider with lowest latency
        let mut best_provider = None;
        let mut best_latency = u128::MAX;

        for status in providers.iter() {
            let cb_state = status.circuit_breaker.lock().unwrap().state;
            if status.is_healthy && cb_state == CircuitBreakerStateName::Closed {
                let tracker = status.latency_tracker.lock().unwrap();
                let avg_latency = if !tracker.is_empty() {
                    tracker.iter().sum::<u128>() / tracker.len() as u128
                } else {
                    u128::MAX
                };

                if avg_latency < best_latency {
                    best_latency = avg_latency;
                    best_provider = Some(status.clone());
                }
            }
        }

        drop(providers);

        if let Some(status) = best_provider {
            let permit = status.semaphore.clone().acquire_owned().await?;
            return Ok((status.provider.as_provider(), permit));
        }

        // Fallback to regular get_next_provider
        self.get_next_provider().await
    }

    /// Manually marks a provider as unhealthy if a real-time request fails.
    pub fn mark_as_unhealthy(&self, provider_to_mark: &Arc<Provider<Http>>) {
        let mut providers_guard = self.providers.lock().unwrap();
        if let Some(status) = providers_guard
            .iter_mut()
            .find(|s| Arc::ptr_eq(&s.provider.as_provider(), provider_to_mark))
        {
            if status.is_healthy {
                warn!(
                    "Provider failed real-time request, marking as unhealthy: {}",
                    status.url
                );
                status.is_healthy = false;
            }
        }
    }

    /// Reports a rate-limit specific error, triggering a short-term backoff for that specific provider.
    pub fn report_rate_limit_error(&self, provider_to_backoff: &Arc<Provider<Http>>) {
        let providers_guard = self.providers.lock().unwrap();
        if let Some(status) = providers_guard
            .iter()
            .find(|s| Arc::ptr_eq(&s.provider.as_provider(), provider_to_backoff))
        {
            let error_count = status
                .rate_limit_errors
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            status.failure_count.fetch_add(1, Ordering::SeqCst);

            // Adaptive concurrency reduction
            self.adjust_concurrency_down(status);

            let backoff_seconds = (1u64 << std::cmp::min(error_count, 6)).saturating_mul(2);
            let backoff_duration = Duration::from_secs(backoff_seconds);

            let mut backoff_until = status.backoff_until.lock().unwrap();
            *backoff_until = Instant::now() + backoff_duration;
            warn!(
                "Rate limit error reported for provider: {}. Error count: {}. Backing off for {:?}",
                status.url, error_count, backoff_duration
            );
        }
    }

    /// Adjust concurrency down due to errors/latency
    fn adjust_concurrency_down(&self, status: &ProviderStatus) {
        let current = status.current_concurrency.load(Ordering::SeqCst);
        let new_concurrency = ((current as f64 * 0.7) as usize).max(1);
        status
            .current_concurrency
            .store(new_concurrency, Ordering::SeqCst);
        warn!(
            "Reducing concurrency for {} from {} to {}",
            status.url, current, new_concurrency
        );
        crate::metrics::gauge_adaptive_concurrency_limit(&status.url, new_concurrency as f64);
    }

    /// Adjust concurrency up due to good performance
    fn adjust_concurrency_up(&self, status: &ProviderStatus) {
        let current = status.current_concurrency.load(Ordering::SeqCst);
        let max_allowed = self.settings.performance.max_concurrent_requests_per_host;
        if current < max_allowed {
            let new_concurrency = ((current as f64 * 1.1) as usize).min(max_allowed);
            status
                .current_concurrency
                .store(new_concurrency, Ordering::SeqCst);
            info!(
                "Increasing concurrency for {} from {} to {}",
                status.url, current, new_concurrency
            );
            crate::metrics::gauge_adaptive_concurrency_limit(&status.url, new_concurrency as f64);
        }
    }

    /// Record successful request with latency tracking
    pub fn report_success(&self, provider: &Arc<Provider<Http>>, latency: Duration) {
        if let Ok(providers) = self.providers.lock() {
            for status in providers.iter() {
                if Arc::ptr_eq(&status.provider.as_provider(), provider) {
                    status.success_count.fetch_add(1, Ordering::SeqCst);

                    // Track latency for adaptive decisions
                    if let Ok(mut tracker) = status.latency_tracker.lock() {
                        tracker.push(latency.as_millis());
                        if tracker.len() > 100 {
                            tracker.remove(0); // Keep recent 100 samples
                        }

                        // Adjust concurrency based on latency performance
                        if tracker.len() >= 20 {
                            let avg_latency = tracker.iter().sum::<u128>() / tracker.len() as u128;
                            let success_rate = {
                                let successes = status.success_count.load(Ordering::SeqCst);
                                let failures = status.failure_count.load(Ordering::SeqCst);
                                if successes + failures > 0 {
                                    successes as f64 / (successes + failures) as f64
                                } else {
                                    1.0
                                }
                            };

                            if avg_latency < 50 && success_rate > 0.95 {
                                // < 50ms average, >95% success
                                self.adjust_concurrency_up(status);
                            } else if avg_latency > 200 || success_rate < 0.8 {
                                // > 200ms or <80% success
                                self.adjust_concurrency_down(status);
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    pub async fn hedged_request<'a, F, Fut, T>(&self, action: F) -> anyhow::Result<T>
    where
        F: Fn(Arc<Provider<Http>>) -> Fut + Send + Sync + Clone + 'a,
        Fut: Future<Output = anyhow::Result<T>> + Send,
        T: Send,
    {
        let s1 = self.get_next_provider_internals().await;
        let s2 = self.get_next_provider_internals().await;

        if let (Ok(p1), Ok(p2)) = (s1, s2) {
            let provider1 = p1.provider.as_provider();
            let provider2 = p2.provider.as_provider();
            if !Arc::ptr_eq(&provider1, &provider2) {
                if let (Ok(permit1), Ok(permit2)) = (
                    p1.semaphore.clone().try_acquire_owned(),
                    p2.semaphore.clone().try_acquire_owned(),
                ) {
                    p1.limiter.until_ready().await;
                    p2.limiter.until_ready().await;

                    let fut1 = (action.clone())(provider1.clone());
                    let fut2 = action(provider2.clone());

                    pin!(fut1);
                    pin!(fut2);

                    let res = tokio::select! {
                        biased;
                        result = &mut fut1 => {
                            if result.is_ok() { self.reset_failures(&provider1); } else { self.report_failure(&provider1); }
                            result
                        },
                        result = &mut fut2 => {
                            if result.is_ok() { self.reset_failures(&provider2); } else { self.report_failure(&provider2); }
                            result
                        },
                    };
                    drop(permit1);
                    drop(permit2);
                    return res;
                }
            }
        }

        // Fallback to single request
        warn!("Hedged request falling back to single request.");
        let (provider, _permit) = self.get_next_provider().await?;
        let result = action(provider.clone()).await;
        if result.is_ok() {
            self.reset_failures(&provider);
        } else {
            self.report_failure(&provider);
        }
        result
    }

    pub fn reset_failures(&self, provider: &Arc<Provider<Http>>) {
        let providers_guard = self.providers.lock().unwrap();
        if let Some(status) = providers_guard
            .iter()
            .find(|s| Arc::ptr_eq(&s.provider.as_provider(), provider))
        {
            let mut cb = status.circuit_breaker.lock().unwrap();
            if cb.state == CircuitBreakerStateName::Closed {
                cb.failures = 0;
            }
        }
    }

    pub fn report_failure(&self, provider: &Arc<Provider<Http>>) {
        let providers_guard = self.providers.lock().unwrap();
        if let Some(status) = providers_guard
            .iter()
            .find(|s| Arc::ptr_eq(&s.provider.as_provider(), provider))
        {
            status.failure_count.fetch_add(1, Ordering::SeqCst);
            let mut cb = status.circuit_breaker.lock().unwrap();
            if cb.state == CircuitBreakerStateName::HalfOpen {
                cb.state = CircuitBreakerStateName::Open;
                cb.last_failure = Some(Instant::now());
                warn!(
                    "Provider {} failed in HalfOpen state. Circuit breaker is now Open.",
                    status.url
                );
                metrics::set_circuit_breaker_state(&status.url, 1.0);
                metrics::increment_circuit_breaker_opened(&status.url);
            } else if cb.state == CircuitBreakerStateName::Closed {
                cb.failures += 1;
                cb.last_failure = Some(Instant::now());
                if cb.failures >= self.settings.rpc.circuit_breaker.failure_threshold {
                    cb.state = CircuitBreakerStateName::Open;
                    warn!(
                        "Provider {} circuit breaker is now Open due to {} consecutive failures.",
                        status.url, cb.failures
                    );
                    metrics::set_circuit_breaker_state(&status.url, 1.0);
                    metrics::increment_circuit_breaker_opened(&status.url);
                }
            }
        }
    }

    /// ✅ FLIGHT RECORDER: Get aggregated resilience statistics
    pub fn get_resilience_stats(&self) -> (f64, u32) {
        let providers_guard = self.providers.lock().unwrap();
        let mut total_success = 0usize;
        let mut total_failures = 0usize;
        let mut circuit_breakers_open = 0u32;

        for status in providers_guard.iter() {
            total_success += status.success_count.load(Ordering::SeqCst);
            total_failures += status.failure_count.load(Ordering::SeqCst);

            let cb = status.circuit_breaker.lock().unwrap();
            if cb.state == CircuitBreakerStateName::Open {
                circuit_breakers_open += 1;
            }
        }

        let rpc_success_rate = if total_success + total_failures > 0 {
            total_success as f64 / (total_success + total_failures) as f64
        } else {
            1.0
        };

        (rpc_success_rate, circuit_breakers_open)
    }
}

impl ProviderStatus {
    async fn adjust_concurrency(&self, settings: &Settings) {
        let latencies: Vec<u128> = {
            let mut tracker = self.latency_tracker.lock().unwrap();
            let values = tracker.clone();
            tracker.clear(); // Clear after cloning for the next window
            values
        };

        if latencies.is_empty() {
            return;
        }

        // p95 latency
        let mut sorted_latencies = latencies;
        sorted_latencies.sort_unstable();
        let p95_index = (sorted_latencies.len() as f64 * 0.95).floor() as usize;
        let p95_latency = sorted_latencies
            .get(p95_index.saturating_sub(1))
            .cloned()
            .unwrap_or_default();

        let old_concurrency = self.current_concurrency.load(Ordering::SeqCst);
        let mut new_concurrency = old_concurrency;

        let rate_limit_errors = self.rate_limit_errors.swap(0, Ordering::SeqCst);

        if rate_limit_errors > 2 && new_concurrency > 1 {
            new_concurrency = (new_concurrency as f64 * 0.7).ceil() as usize;
            warn!(
                "Provider {} hit {} rate limit errors. Reducing concurrency to {}.",
                self.url, rate_limit_errors, new_concurrency
            );
        } else if p95_latency > 1500 && new_concurrency > 1 {
            // 1.5s p95 latency threshold
            new_concurrency -= 1;
            info!(
                "Provider {} high p95 latency ({:?}ms). Decreasing concurrency to {}.",
                self.url, p95_latency, new_concurrency
            );
        } else if p95_latency < 400
            && new_concurrency < settings.performance.max_concurrent_requests_per_host * 2
        {
            // Allow up to 2x configured max
            new_concurrency += 1;
            info!(
                "Provider {} low p95 latency ({:?}ms). Increasing concurrency to {}.",
                self.url, p95_latency, new_concurrency
            );
        }

        if old_concurrency != new_concurrency {
            self.current_concurrency
                .store(new_concurrency, Ordering::SeqCst);

            // Handle increase vs decrease separately to avoid underflow
            if new_concurrency > old_concurrency {
                let permits_to_add = new_concurrency - old_concurrency;
                self.semaphore.add_permits(permits_to_add);
            }
            // For decrease, we don't need to remove permits - just let them drain naturally
            // The semaphore will respect the new current_concurrency value

            info!(
                "Provider {} concurrency limit adjusted from {} to {}.",
                self.url, old_concurrency, new_concurrency
            );
            metrics::set_adaptive_concurrency(&self.url, new_concurrency as f64);
        }
    }
}
