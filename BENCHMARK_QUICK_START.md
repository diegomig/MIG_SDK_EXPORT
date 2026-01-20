# Quick Start: Ejecutar Benchmark con Optimizaciones P0/P1

## ✅ Verificación Pre-Benchmark (2 minutos)

### 1. Verificar Configuración

Las optimizaciones P0/P1 están habilitadas por defecto en el código. Verificar en `Config.toml`:

```toml
[performance]
jit_cache_ttl_ms = 60000  # ✅ P0: TTL configurado
jit_cache_tolerance_blocks = 3  # ✅ P0: Fuzzy matching
```

Las siguientes configuraciones están en código (valores por defecto):
- `parallel_price_fetching_enabled = true` (P1)
- `price_fetch_chunk_size = 20` (P1)
- TTL diferenciado: 30s touched / 300s untouched (P0)

### 2. Variables de Entorno

```bash
# Requerido
export DATABASE_URL="postgresql://user:pass@localhost:5432/mig_topology"

# Opcional pero recomendado para cache
export REDIS_URL="redis://localhost:6379"

# RPC endpoints
export SDK_RPC_HTTP_URLS="https://arb1.arbitrum.io/rpc,https://..."
export SDK_RPC_WS_URLS="wss://arb1.arbitrum.io/ws,..."
```

## 🚀 Ejecutar Benchmark

```bash
# Con Redis (recomendado para cache hit rate)
cargo run --example benchmark_metrics --features redis,observability

# Sin Redis (cache hit rate será 0%)
cargo run --example benchmark_metrics --features observability
```

## 📊 Qué Buscar en los Resultados

### Métricas Clave (objetivos P0/P1)

1. **Cache Hit Rate**: Debe ser ≥80%
   - Buscar en logs: `Cache Hits: X, Cache Hit Rate: Y%`
   - Si es 0%, verificar Redis está corriendo

2. **JIT Latency**: Debe ser ≤100ms (remote RPC)
   - Buscar en logs: `[JIT] Fetch duration: Xms`
   - Debería ver reducción significativa vs baseline

3. **RPC Calls per Block**: Debe ser ≤30
   - Buscar en logs: `Total RPC calls: X`
   - Debería ser mucho menor que baseline (~158)

4. **End-to-End Latency**: Debe ser ≤200ms
   - Sumar todas las fases del ciclo completo
   - Buscar en logs: `PhaseEnd` events con `duration_ms`

### Logs de Optimizaciones P1

Buscar estos mensajes en los logs:

```
✅ [P1] Parallel price fetch completed: X prices from Y successful chunks
✅ [P1] Batch updated Z graph weights in database
```

### Logs de Optimizaciones P0

Buscar estos mensajes:

```
✅ Hot Pool Manager populated with X pools
✅ [JIT] Cache hit for pool (state hash match)
```

## 📁 Archivos de Resultados

- **Flight Recorder**: `benchmarks/flight_recorder_YYYYMMDD_HHMMSS.jsonl`
- **Métricas resumidas**: Al final de la ejecución en consola

## ⚠️ Troubleshooting

### Cache Hit Rate = 0%
- Verificar Redis está corriendo: `docker ps | grep redis`
- Verificar `REDIS_URL` está configurado
- Verificar conexión: `redis-cli ping`

### RPC Calls muy altos
- Verificar que Hot Pool Manager está poblado
- Verificar que cache está funcionando (logs de cache hits)
- Revisar configuración de `jit_cache_ttl_ms`

### Latencia alta
- Verificar RPC endpoints son rápidos
- Verificar que parallel fetching está funcionando (logs P1)
- Revisar batch DB updates están funcionando (logs P1)

## 📝 Análisis Post-Benchmark

Ver `docs/benchmarks/notes/BENCHMARK_CHECKLIST.md` para análisis detallado de resultados.
