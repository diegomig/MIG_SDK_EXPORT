# Verificación de Métricas P0/P1 en Flight Recorder

## ✅ Métricas Actualmente Capturadas

### Cache Events (P0)
- **Tipo**: `CacheEvent`
- **Campos**: `cache_type`, `event_type` ("hit"/"miss"), `key`, `block`
- **Ubicación**: `src/flight_recorder.rs:116-122`
- **Uso**: Capturado en `jit_state_fetcher.rs` y `graph_service.rs`

### PhaseEnd Events (P0/P1)
- **Tipo**: `PhaseEnd`
- **Campos**: `phase`, `duration_ms`, `result` (JSON con metadata), `block`
- **Metadata agregada**:
  - ✅ Batch DB update: `batch_db_update: true`, `weights_batch_size`
  - ✅ Parallel price fetch: `parallel_price_fetch_enabled`, `price_fetch_chunk_size`
  - ✅ Tokens/prices: `unique_tokens`, `prices_loaded`

### RpcCall Events (P0)
- **Tipo**: `RpcCall`
- **Campos**: `endpoint`, `method`, `duration_ms`, `success`, `pools_requested`, `pools_returned`
- **Uso**: Para medir reducción de RPC calls

## 📊 Métricas que se Pueden Extraer del Benchmark

### Desde PhaseEnd Events

#### `graph_updates` Phase
```json
{
  "mode": "incremental" | "full_refresh",
  "pools_updated": <number>,
  "batch_db_update": true,
  "weights_batch_size": <number>,
  "parallel_price_fetch_enabled": true,
  "price_fetch_chunk_size": 20,
  "unique_tokens": <number>,
  "prices_loaded": <number>
}
```

**Métricas derivadas**:
- Batch update efficiency: `weights_batch_size` vs individual updates
- Parallel fetch efficiency: `prices_loaded` / `unique_tokens` ratio
- Time saved: Comparar `duration_ms` con/sin optimizaciones

#### `jit_fetch_internal` Phase
**Metadata actual**: Ya incluye cache hits/misses, pools requested/returned

**Métricas derivadas**:
- Cache hit rate: `cache_hits / (cache_hits + cache_misses)`
- JIT latency: `duration_ms`
- RPC reduction: `pools_requested` vs `pools_returned` (cache hits)

### Desde CacheEvent

**Métricas**:
- Total cache hits: Contar `event_type == "hit"`
- Total cache misses: Contar `event_type == "miss"`
- Cache hit rate: `hits / (hits + misses) * 100`

### Desde RpcCall

**Métricas**:
- Total RPC calls: Contar eventos `RpcCall`
- RPC calls per block: `total_calls / blocks_processed`
- Average latency: `avg(duration_ms)`
- Success rate: `success_count / total_calls`

## 🔍 Análisis Post-Benchmark

### Scripts de Análisis

```bash
# Cache hit rate
cat benchmarks/flight_recorder_*.jsonl | \
  jq 'select(.type=="CacheEvent" and .event_type=="hit")' | wc -l

# Batch DB updates
cat benchmarks/flight_recorder_*.jsonl | \
  jq 'select(.type=="PhaseEnd" and .phase=="graph_updates" and .result.batch_db_update==true) | .result.weights_batch_size'

# Parallel price fetch
cat benchmarks/flight_recorder_*.jsonl | \
  jq 'select(.type=="PhaseEnd" and .phase=="graph_updates") | .result.parallel_price_fetch_enabled'

# RPC calls per block
TOTAL_CALLS=$(cat benchmarks/flight_recorder_*.jsonl | jq 'select(.type=="RpcCall")' | wc -l)
BLOCKS=$(cat benchmarks/flight_recorder_*.jsonl | jq 'select(.type=="BlockEnd")' | wc -l)
echo "RPC calls per block: $((TOTAL_CALLS / BLOCKS))"
```

## ⚠️ Métricas Faltantes (Opcionales)

### P0 Optimizations
- **Merkle cache validation**: No capturado explícitamente (pero se puede inferir de cache hits)
- **TTL differentiation**: No capturado (pero se puede inferir de cache hits con timestamps)
- **Hot Pool Manager pre-warming**: No capturado explícitamente

### P1 Optimizations
- **Parallel fetch chunks**: Capturado parcialmente (chunk_size, pero no número de chunks)
- **Batch update chunks**: No capturado (pero weights_batch_size indica tamaño)

## ✅ Conclusión

Las métricas principales están capturadas:
- ✅ Cache hit rate (CacheEvent)
- ✅ Batch DB updates (PhaseEnd metadata)
- ✅ Parallel price fetch (PhaseEnd metadata)
- ✅ RPC calls reduction (RpcCall count)
- ✅ JIT latency (PhaseEnd duration_ms)

Las métricas faltantes son opcionales y se pueden inferir de las existentes.
