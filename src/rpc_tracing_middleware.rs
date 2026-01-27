// RPC Tracing Utilities - Funciones helper para tracing de llamadas RPC
//
// NOTA: Debido a problemas complejos de tipos y lifetimes con el trait Middleware de ethers-rs,
// usamos un enfoque pragmático: tracing directo en los puntos de llamada (ver rpc_pool.rs).
//
// Este archivo contiene funciones helper para estimar costos CU y logging consistente.

use crate::metrics;

/// Estima el costo CU basado en el método RPC y el tamaño del payload
pub fn estimate_cu_cost(method: &str, payload_size: usize) -> f64 {
    match method {
        "eth_blockNumber" => 0.1,
        "eth_call" => 0.1,
        "eth_getBlockByNumber" => 0.1,
        "eth_getLogs" => {
            // Base cost: 75 CU (Alchemy) - MUY CARO
            let base = 75.0;
            let size_multiplier = (payload_size as f64 / 1024.0).max(1.0); // KB
            base * size_multiplier
        }
        "eth_estimateGas" => 0.1,
        "eth_sendTransaction" => 0.1,
        _ => 0.1, // Default estimate para métodos desconocidos
    }
}

/// Registra métricas para una llamada RPC
pub fn record_rpc_call(
    component: &str,
    method: &str,
    payload_size: usize,
    duration: std::time::Duration,
) {
    let cu_cost = estimate_cu_cost(method, payload_size);
    metrics::increment_rpc_call(component);
    metrics::increment_rpc_call_by_method(component, method);
    metrics::record_rpc_cu_cost(component, method, cu_cost);
    metrics::record_rpc_payload_size(component, method, payload_size);
    metrics::record_rpc_call_latency(component, method, duration);
}
