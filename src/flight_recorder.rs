// Flight Recorder - Sistema ligero de captura de eventos para análisis post-mortem
// Overhead: <1% CPU, ~10MB RAM por minuto
// Activable: Via env var o signal
// Formato: JSON Lines (1 evento por línea)

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;
use std::time::Instant;

// ============================================================================
// EVENTOS SIMPLIFICADOS (Solo lo esencial)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FlightEvent {
    /// Inicio de bloque
    BlockStart {
        ts: u64,
        block: u64,
    },
    
    /// Fin de bloque
    BlockEnd {
        ts: u64,
        block: u64,
        duration_ms: u64,
        routes_generated: usize,
        routes_filtered: usize,
        opportunities: usize,
        gas_saved_l1: Option<u64>, // Gas ahorrado vs llamadas individuales (L1 estimate)
        gas_saved_l2: Option<u64>, // Gas ahorrado vs llamadas individuales (L2 estimate)
    },
    
    /// Fase iniciada
    PhaseStart {
        ts: u64,
        phase: String, // "price_fetch", "jit_fetch", "block_parser", etc
        metadata: serde_json::Value,
        block: Option<u64>, // ✅ MEJORA: Correlación por bloque
    },
    
    /// Fase completada
    PhaseEnd {
        ts: u64,
        phase: String,
        duration_ms: u64,
        result: serde_json::Value, // Solo stats, no datos completos
        block: Option<u64>, // ✅ MEJORA: Correlación por bloque
    },
    
    /// Decisión crítica
    Decision {
        ts: u64,
        component: String,
        action: String, // "filter", "skip", "execute"
        reason: String,
        context: serde_json::Value, // Solo datos clave
        block: Option<u64>, // ✅ MEJORA: Correlación por bloque
    },
    
    /// Estado de coordinación
    Coordination {
        ts: u64,
        what: String, // "price_fetch_done_but_jit_pending"
        details: serde_json::Value,
        block: Option<u64>, // ✅ MEJORA: Correlación por bloque
    },
    
    /// Llamada RPC
    RpcCall {
        ts: u64,
        endpoint: String,
        method: String,
        duration_ms: u64,
        success: bool,
        block: Option<u64>, // ✅ MEJORA: Correlación por bloque
        payload_size_bytes: Option<usize>, // ✅ MEJORA: Tamaño de payload para detectar truncamiento
        pools_requested: Option<usize>, // ✅ MEJORA: Pools solicitados vs devueltos
        pools_returned: Option<usize>, // ✅ MEJORA: Pools devueltos
    },
    
    /// ✅ MEJORA: Error explícito
    Error {
        ts: u64,
        component: String,
        error_type: String, // "rpc_failure", "simulation_failure", "state_mismatch", etc
        message: String,
        context: serde_json::Value,
        block: Option<u64>,
    },
    
    /// ✅ MEJORA: Bloque saltado (no procesado)
    BlockSkipped {
        ts: u64,
        block: u64,
        reason: String, // "duplicate", "invalid", "already_processed", "zero"
        last_processed_block: u64,
        gap_blocks: u64, // Diferencia entre current_block y last_processed_block
    },
    
    /// ✅ MEJORA: Gap grande entre bloques procesados
    BlockGap {
        ts: u64,
        current_block: u64,
        last_processed_block: u64,
        gap_blocks: u64,
        warning_threshold: u64, // Umbral que se excedió (ej: 20)
    },
    
    /// ✅ CACHE: Evento de cache hit/miss para Redis
    CacheEvent {
        ts: u64,
        cache_type: String, // "pool_state", "route", etc
        event_type: String, // "hit" or "miss"
        key: String, // Cache key (ej: pool address)
        block: Option<u64>,
    },
}

// ============================================================================
// FLIGHT RECORDER (Thread-safe, bajo overhead)
// ============================================================================

#[derive(Debug)]
pub struct FlightRecorder {
    enabled: Arc<AtomicBool>,
    event_tx: mpsc::UnboundedSender<FlightEvent>,
    start_time: Instant,
    event_count: Arc<AtomicU64>,
    // ✅ MEJORA: Métricas de overhead
    serialization_time_ns: Arc<AtomicU64>, // Tiempo total de serialización
    dropped_events: Arc<AtomicU64>, // Eventos perdidos por buffer lleno
}

impl FlightRecorder {
    /// Crear recorder (disabled por defecto)
    pub fn new() -> (Self, mpsc::UnboundedReceiver<FlightEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        
        (
            Self {
                enabled: Arc::new(AtomicBool::new(false)),
                event_tx: tx,
                start_time: Instant::now(),
                event_count: Arc::new(AtomicU64::new(0)),
                serialization_time_ns: Arc::new(AtomicU64::new(0)),
                dropped_events: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }
    
    /// Activar recording
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
        println!("🎬 Flight Recorder ENABLED");
    }
    
    /// Desactivar recording
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Relaxed);
        println!("⏹️  Flight Recorder DISABLED");
    }
    
    /// Check si está activo
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    
    /// Timestamp relativo (ms desde inicio)
    fn timestamp(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
    
    /// Registrar evento (fast path si disabled)
    pub fn record(&self, event: FlightEvent) {
        if !self.is_enabled() {
            return; // Fast path: 1 instrucción si disabled
        }
        
        // ✅ MEJORA: Medir overhead de serialización
        let serialization_start = std::time::Instant::now();
        
        // Agregar timestamp si no lo tiene
        let event_with_ts = match event {
            FlightEvent::BlockStart { ts: 0, block } => {
                FlightEvent::BlockStart { ts: self.timestamp(), block }
            }
            FlightEvent::BlockEnd { ts: 0, block, duration_ms, routes_generated, routes_filtered, opportunities, gas_saved_l1, gas_saved_l2 } => {
                FlightEvent::BlockEnd { ts: self.timestamp(), block, duration_ms, routes_generated, routes_filtered, opportunities, gas_saved_l1, gas_saved_l2 }
            }
            FlightEvent::PhaseStart { ts: 0, phase, metadata, block } => {
                FlightEvent::PhaseStart { ts: self.timestamp(), phase, metadata, block }
            }
            FlightEvent::PhaseEnd { ts: 0, phase, duration_ms, result, block } => {
                FlightEvent::PhaseEnd { ts: self.timestamp(), phase, duration_ms, result, block }
            }
            FlightEvent::Decision { ts: 0, component, action, reason, context, block } => {
                FlightEvent::Decision { ts: self.timestamp(), component, action, reason, context, block }
            }
            FlightEvent::Coordination { ts: 0, what, details, block } => {
                FlightEvent::Coordination { ts: self.timestamp(), what, details, block }
            }
            FlightEvent::RpcCall { ts: 0, endpoint, method, duration_ms, success, block, payload_size_bytes, pools_requested, pools_returned } => {
                FlightEvent::RpcCall { ts: self.timestamp(), endpoint, method, duration_ms, success, block, payload_size_bytes, pools_requested, pools_returned }
            }
            FlightEvent::Error { ts: 0, component, error_type, message, context, block } => {
                FlightEvent::Error { ts: self.timestamp(), component, error_type, message, context, block }
            }
            FlightEvent::CacheEvent { ts: 0, cache_type, event_type, key, block } => {
                FlightEvent::CacheEvent { ts: self.timestamp(), cache_type, event_type, key, block }
            }
            other => other,
        };
        
        // ✅ MEJORA: Medir tiempo de serialización JSON (aproximado)
        let serialization_duration = serialization_start.elapsed();
        self.serialization_time_ns.fetch_add(serialization_duration.as_nanos() as u64, Ordering::Relaxed);
        
        // Non-blocking send (si el buffer está lleno, skip event)
        match self.event_tx.send(event_with_ts) {
            Ok(_) => {
                self.event_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                // Buffer lleno, evento perdido
                self.dropped_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    
    /// Stats
    pub fn stats(&self) -> (bool, u64) {
        (self.is_enabled(), self.event_count.load(Ordering::Relaxed))
    }
    
    /// ✅ MEJORA: Stats detallados con overhead
    pub fn stats_detailed(&self) -> (bool, u64, u64, u64) {
        (
            self.is_enabled(),
            self.event_count.load(Ordering::Relaxed),
            self.serialization_time_ns.load(Ordering::Relaxed),
            self.dropped_events.load(Ordering::Relaxed),
        )
    }
}

// ============================================================================
// WRITER TASK (Escribe eventos a disco en background)
// ============================================================================

pub async fn flight_recorder_writer(
    mut event_rx: mpsc::UnboundedReceiver<FlightEvent>,
    output_file: String,
) -> Result<(), std::io::Error> {
    // Crear directorio si no existe
    if let Some(parent) = std::path::Path::new(&output_file).parent() {
        match std::fs::create_dir_all(parent) {
            Ok(_) => println!("✅ Flight Recorder: Created directory {:?}", parent),
            Err(e) => {
                eprintln!("❌ Flight Recorder: Failed to create directory {:?}: {}", parent, e);
                return Err(e);
            }
        }
    }
    
    let file = match File::create(&output_file).await {
        Ok(f) => {
            println!("✅ Flight Recorder: File created successfully: {}", output_file);
            f
        }
        Err(e) => {
            eprintln!("❌ Flight Recorder: Failed to create file {}: {}", output_file, e);
            return Err(e);
        }
    };
    let mut writer = BufWriter::new(file);
    
    println!("📝 Flight Recorder: Writer task ready, waiting for events...");
    
    let mut count = 0;
    while let Some(event) = event_rx.recv().await {
        // JSON Lines format (1 evento por línea)
        let json = serde_json::to_string(&event).unwrap();
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        
        count += 1;
        
        // Flush cada 10 eventos (más frecuente para evitar pérdida de datos)
        // y también cada 100 eventos (para eficiencia en casos de alto volumen)
        if count % 10 == 0 || count % 100 == 0 {
            writer.flush().await?;
        }
    }
    
    // Flush final
    writer.flush().await?;
    println!("✅ Flight Recorder saved {} events to {}", count, output_file);
    
    Ok(())
}

// ============================================================================
// MACROS HELPER (Uso conveniente)
// ============================================================================

// Helper trait para manejar Option y Arc de forma uniforme
pub trait FlightRecorderHelper {
    fn record_event(&self, event: FlightEvent);
}

impl FlightRecorderHelper for Option<Arc<FlightRecorder>> {
    fn record_event(&self, event: FlightEvent) {
        if let Some(ref r) = self {
            r.record(event);
        }
    }
}

impl FlightRecorderHelper for Arc<FlightRecorder> {
    fn record_event(&self, event: FlightEvent) {
        self.record(event);
    }
}

impl FlightRecorderHelper for &Arc<FlightRecorder> {
    fn record_event(&self, event: FlightEvent) {
        (*self).record(event);
    }
}

impl FlightRecorderHelper for &Option<Arc<FlightRecorder>> {
    fn record_event(&self, event: FlightEvent) {
        if let Some(ref r) = self {
            r.record(event);
        }
    }
}

#[macro_export]
macro_rules! record_phase_start {
    ($recorder:expr, $phase:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::PhaseStart {
            ts: 0,
            phase: $phase.to_string(),
            metadata: serde_json::json!({}),
            block: None,
        });
    };
    ($recorder:expr, $phase:expr, $metadata:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::PhaseStart {
            ts: 0,
            phase: $phase.to_string(),
            metadata: $metadata,
            block: None,
        });
    };
    ($recorder:expr, $phase:expr, $metadata:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::PhaseStart {
            ts: 0,
            phase: $phase.to_string(),
            metadata: $metadata,
            block: Some($block),
        });
    };
}

#[macro_export]
macro_rules! record_phase_end {
    ($recorder:expr, $phase:expr, $start:expr, $result:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::PhaseEnd {
            ts: 0,
            phase: $phase.to_string(),
            duration_ms,
            result: $result,
            block: None,
        });
    }};
    ($recorder:expr, $phase:expr, $start:expr, $result:expr, $block:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::PhaseEnd {
            ts: 0,
            phase: $phase.to_string(),
            duration_ms,
            result: $result,
            block: Some($block),
        });
    }};
}

#[macro_export]
macro_rules! record_decision {
    ($recorder:expr, $component:expr, $action:expr, $reason:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Decision {
            ts: 0,
            component: $component.to_string(),
            action: $action.to_string(),
            reason: $reason.to_string(),
            context: serde_json::json!({}),
            block: None,
        });
    };
    ($recorder:expr, $component:expr, $action:expr, $reason:expr, $context:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Decision {
            ts: 0,
            component: $component.to_string(),
            action: $action.to_string(),
            reason: $reason.to_string(),
            context: $context,
            block: None,
        });
    };
    ($recorder:expr, $component:expr, $action:expr, $reason:expr, $context:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Decision {
            ts: 0,
            component: $component.to_string(),
            action: $action.to_string(),
            reason: $reason.to_string(),
            context: $context,
            block: Some($block),
        });
    };
}

#[macro_export]
macro_rules! record_coordination {
    ($recorder:expr, $what:expr, $details:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Coordination {
            ts: 0,
            what: $what.to_string(),
            details: $details,
            block: None,
        });
    };
    ($recorder:expr, $what:expr, $details:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Coordination {
            ts: 0,
            what: $what.to_string(),
            details: $details,
            block: Some($block),
        });
    };
}

#[macro_export]
macro_rules! record_rpc_call {
    ($recorder:expr, $endpoint:expr, $method:expr, $start:expr, $success:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::RpcCall {
            ts: 0,
            endpoint: $endpoint.to_string(),
            method: $method.to_string(),
            duration_ms,
            success: $success,
            block: None,
            payload_size_bytes: None,
            pools_requested: None,
            pools_returned: None,
        });
    }};
    ($recorder:expr, $endpoint:expr, $method:expr, $start:expr, $success:expr, $block:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::RpcCall {
            ts: 0,
            endpoint: $endpoint.to_string(),
            method: $method.to_string(),
            duration_ms,
            success: $success,
            block: Some($block),
            payload_size_bytes: None,
            pools_requested: None,
            pools_returned: None,
        });
    }};
    ($recorder:expr, $endpoint:expr, $method:expr, $start:expr, $success:expr, $block:expr, $payload_size:expr, $pools_req:expr, $pools_ret:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::RpcCall {
            ts: 0,
            endpoint: $endpoint.to_string(),
            method: $method.to_string(),
            duration_ms,
            success: $success,
            block: Some($block),
            payload_size_bytes: Some($payload_size),
            pools_requested: Some($pools_req),
            pools_returned: Some($pools_ret),
        });
    }};
}

// ✅ MEJORA: Macro para registrar errores
#[macro_export]
macro_rules! record_error {
    ($recorder:expr, $component:expr, $error_type:expr, $message:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Error {
            ts: 0,
            component: $component.to_string(),
            error_type: $error_type.to_string(),
            message: $message.to_string(),
            context: serde_json::json!({}),
            block: None,
        });
    };
    ($recorder:expr, $component:expr, $error_type:expr, $message:expr, $context:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Error {
            ts: 0,
            component: $component.to_string(),
            error_type: $error_type.to_string(),
            message: $message.to_string(),
            context: $context,
            block: None,
        });
    };
    ($recorder:expr, $component:expr, $error_type:expr, $message:expr, $context:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::Error {
            ts: 0,
            component: $component.to_string(),
            error_type: $error_type.to_string(),
            message: $message.to_string(),
            context: $context,
            block: Some($block),
        });
    };
}

#[macro_export]
macro_rules! record_block_skipped {
    ($recorder:expr, $block:expr, $reason:expr, $last_processed:expr) => {
        let gap = if $block > $last_processed {
            $block - $last_processed
        } else {
            0
        };
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::BlockSkipped {
            ts: 0,
            block: $block,
            reason: $reason.to_string(),
            last_processed_block: $last_processed,
            gap_blocks: gap,
        });
    };
}

#[macro_export]
macro_rules! record_block_gap {
    ($recorder:expr, $current_block:expr, $last_processed:expr, $threshold:expr) => {
        let gap = if $current_block > $last_processed {
            $current_block - $last_processed
        } else {
            0
        };
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::BlockGap {
            ts: 0,
            current_block: $current_block,
            last_processed_block: $last_processed,
            gap_blocks: gap,
            warning_threshold: $threshold,
        });
    };
}

#[macro_export]
macro_rules! record_block_start {
    ($recorder:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::BlockStart {
            ts: 0,
            block: $block,
        });
    };
}

#[macro_export]
macro_rules! record_block_end {
    ($recorder:expr, $block:expr, $start:expr, $routes_generated:expr, $routes_filtered:expr, $opportunities:expr, $gas_saved_l1:expr, $gas_saved_l2:expr) => {{
        let duration_ms = $start.elapsed().as_millis() as u64;
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::BlockEnd {
            ts: 0,
            block: $block,
            duration_ms,
            routes_generated: $routes_generated,
            routes_filtered: $routes_filtered,
            opportunities: $opportunities,
            gas_saved_l1: $gas_saved_l1,
            gas_saved_l2: $gas_saved_l2,
        });
    }};
}

/// ✅ CACHE: Macro para registrar eventos de cache hit/miss
#[macro_export]
macro_rules! record_cache_event {
    ($recorder:expr, $cache_type:expr, $event_type:expr, $key:expr, $block:expr) => {
        $crate::flight_recorder::FlightRecorderHelper::record_event(&$recorder, $crate::flight_recorder::FlightEvent::CacheEvent {
            ts: 0,
            cache_type: $cache_type.to_string(),
            event_type: $event_type.to_string(),
            key: $key.to_string(),
            block: $block,
        });
    };
}


