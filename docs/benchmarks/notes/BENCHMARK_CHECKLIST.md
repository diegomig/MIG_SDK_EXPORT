# Benchmark Checklist - P0/P1 Optimizations

## Pre-Benchmark Verification

### ✅ Configuración de Optimizaciones

Verificar que las siguientes optimizaciones estén habilitadas en `Config.toml`:

- [ ] `parallel_price_fetching_enabled = true` (P1)
- [ ] `price_fetch_chunk_size = 20` (P1, valor razonable)
- [ ] TTL diferenciado configurado (P0):
  - [ ] `touched_pool_ttl_seconds = 30`
  - [ ] `untouched_pool_ttl_seconds = 300`
- [ ] Fuzzy block matching habilitado (P0)
- [ ] Hot Pool Manager habilitado (P0)

### ✅ Variables de Entorno

- [ ] `DATABASE_URL` configurado
- [ ] `REDIS_URL` configurado (opcional pero recomendado para cache)
- [ ] `SDK_RPC_HTTP_URLS` con endpoints válidos
- [ ] `SDK_RPC_WS_URLS` con endpoints válidos (opcional)

## Métricas a Revisar en el Benchmark

### 🎯 Objetivos P0/P1

#### Cache Hit Rate
- **Objetivo**: ≥80%
- **Cómo medir**: Revisar métricas de `redis_cache_hits` vs `redis_cache_misses`
- **Ubicación**: `benchmark_metrics.rs` líneas 861-864

#### JIT Latency
- **Objetivo**: 
  - Local node: ≤10ms
  - Remote RPC: ≤100ms
- **Cómo medir**: Revisar eventos `PhaseEnd` con `phase == "jit_state_fetch"`
- **Ubicación**: Flight Recorder events

#### End-to-End Latency
- **Objetivo**: ≤200ms (discovery → graph update)
- **Cómo medir**: Suma de latencias de todas las fases en un ciclo completo
- **Ubicación**: Flight Recorder events con `phase == "discovery"` y `phase == "graph_updates"`

#### RPC Calls per Block
- **Objetivo**: ≤30 calls/block (>80% reducción)
- **Cómo medir**: Contar eventos `RpcCall` en Flight Recorder
- **Ubicación**: Flight Recorder events con `event_type == "RpcCall"`

### 📊 Métricas Adicionales

#### Batch DB Updates (P1)
- **Qué buscar**: Reducción en tiempo de actualización de weights
- **Cómo medir**: Comparar tiempo de `graph_updates` phase antes/después
- **Indicador**: Debería ser más rápido con batch updates

#### Parallel Price Fetching (P1)
- **Qué buscar**: Reducción en tiempo de price fetching
- **Cómo medir**: Revisar logs de `[P1] Parallel price fetch completed`
- **Indicador**: Debería ver múltiples chunks procesándose en paralelo

#### Cache Invalidation (P0)
- **Qué buscar**: Cache hits cuando el estado no cambió
- **Cómo medir**: Revisar eventos `CacheEvent` con `cache_hit == true`
- **Indicador**: Cache válido incluso después de varios bloques si el estado no cambió

#### Hot Pool Manager (P0)
- **Qué buscar**: Pre-warming de cache con top-K pools
- **Cómo medir**: Revisar logs de `Hot Pool Manager populated`
- **Indicador**: Cache hits inmediatos para pools populares

## Ejecutar Benchmark

```bash
# Con todas las features habilitadas
cargo run --example benchmark_metrics --features redis,observability

# O sin Redis (cache hit rate será 0%)
cargo run --example benchmark_metrics --features observability
```

## Análisis de Resultados

### ✅ Si los objetivos se cumplen:
1. Documentar métricas en `docs/BENCHMARKS.md`
2. Actualizar `PRODUCTION_READINESS.md` con estado de optimizaciones
3. Preparar reporte para grants

### ⚠️ Si hay problemas:
1. Revisar logs de Flight Recorder para identificar cuellos de botella
2. Verificar configuración de optimizaciones
3. Ejecutar tests específicos para el componente problemático
4. Revisar métricas detalladas en `benchmarks/flight_recorder_*.jsonl`

## Archivos de Resultados

- **Flight Recorder**: `benchmarks/flight_recorder_YYYYMMDD_HHMMSS.jsonl`
- **Métricas resumidas**: Salida en consola al finalizar el benchmark
- **Logs**: Salida estándar con métricas en tiempo real

## Comandos Útiles para Análisis

```bash
# Analizar Flight Recorder JSONL
cat benchmarks/flight_recorder_*.jsonl | jq 'select(.event_type=="RpcCall")' | wc -l

# Contar cache hits
cat benchmarks/flight_recorder_*.jsonl | jq 'select(.event_type=="CacheEvent" and .cache_hit==true)' | wc -l

# Ver latencias JIT
cat benchmarks/flight_recorder_*.jsonl | jq 'select(.phase=="jit_state_fetch") | .duration_ms'
```
