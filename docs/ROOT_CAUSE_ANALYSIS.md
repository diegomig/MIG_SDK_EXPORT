# Análisis de Causa Raíz - Problemas de Performance

**Fecha**: 2026-01-16  
**Benchmark**: benchmark_report_20260116_135405.md

## Problemas Identificados

### 1. ❌ eth_getLogs NO se registra en Flight Recorder

**Síntoma**: Solo se registran 14 llamadas `eth_blockNumber`, ninguna `eth_getLogs`.

**Causa Raíz**: 
- Los adapters (UniswapV2, UniswapV3, etc.) usan `event_filter.query().await` que internamente llama a `provider.get_logs()` directamente
- Estas llamadas NO pasan por `RpcPool::get_logs_with_recording()`
- El Flight Recorder nunca ve estas llamadas porque están fuera del sistema de recording

**Ubicación del problema**:
- `src/adapters/uniswap_v2.rs:72` - `event_filter.query().await`
- `src/adapters/uniswap_v3.rs:64` - Similar pattern
- Todos los adapters usan el mismo patrón

**Solución**:
- Modificar los adapters para usar `RpcPool::get_logs_with_recording()` en lugar de `event_filter.query()`
- O crear un wrapper que intercepte las llamadas `get_logs()` del provider

---

### 2. ❌ PostgresAsyncWriter NO se usa en el loop principal de discovery

**Síntoma**: Throughput bajo (0.04 blocks/sec), posiblemente por bloqueos en DB writes.

**Causa Raíz**:
- En `src/orchestrator.rs:1055`, el código usa `database::upsert_pool()` directamente (síncrono)
- Solo Curve usa `db_writer.upsert_pool_full()` (línea 935)
- El loop principal de discovery procesa pools uno por uno con writes síncronos, causando bloqueos

**Ubicación del problema**:
- `src/orchestrator.rs:1055` - `database::upsert_pool()` en lugar de `db_writer.upsert_pool_full()`
- `src/orchestrator.rs:1144` - `database::set_pool_activity()` en lugar de `db_writer.batch_set_pool_activity()`

**Solución**:
- Reemplazar todas las llamadas directas a `database::upsert_pool()` con `db_writer.upsert_pool_full()`
- Reemplazar todas las llamadas directas a `database::set_pool_activity()` con `db_writer.batch_set_pool_activity()`

---

### 3. ❌ Redis Cache Hit Rate 0%

**Síntoma**: Cache hit rate = 0% en todos los ciclos.

**Causa Raíz**:
- `GraphService::fetch_pool_states()` tiene lógica de Redis, pero:
  1. Los pools se cachean DESPUÉS de ser fetchados (línea 489)
  2. En el primer ciclo, no hay nada en cache, por lo que hit rate = 0%
  3. En ciclos subsecuentes, los pools pueden haber cambiado de estado, por lo que el cache puede estar desactualizado
  4. El TTL de Redis es de 10 segundos (línea 203 de benchmark_metrics.rs), pero los ciclos duran ~51 segundos

**Ubicación del problema**:
- `src/graph_service.rs:442-498` - Lógica de Redis cache
- `examples/benchmark_metrics.rs:203` - TTL de 10 segundos

**Solución**:
- Aumentar el TTL de Redis a al menos 60 segundos (más que la duración de un ciclo)
- Verificar que Redis esté realmente conectado y funcionando
- Agregar métricas para verificar si Redis está siendo usado

---

### 4. ❌ Throughput bajo (0.04 blocks/sec vs objetivo 4 blocks/sec)

**Síntoma**: Throughput de 0.04 blocks/sec, 100x más lento que el objetivo.

**Causa Raíz**:
- Combinación de múltiples problemas:
  1. DB writes síncronos bloqueando el loop (Problema #2)
  2. Solo se procesan 4 bloques por ciclo (línea 966 de orchestrator.rs)
  3. Cada ciclo dura ~51 segundos, procesando solo 4 bloques = 0.08 blocks/sec teórico máximo
  4. Con overhead, el throughput real es 0.04 blocks/sec

**Ubicación del problema**:
- `src/orchestrator.rs:966` - `let range = 4;` - Solo procesa 4 bloques por ciclo
- `src/orchestrator.rs:1055` - DB writes síncronos bloqueando

**Solución**:
- Usar `PostgresAsyncWriter` para todos los DB writes (Problema #2)
- Aumentar el rango de bloques procesados por ciclo (de 4 a al menos 20-40 bloques)
- Optimizar el procesamiento de bloques para reducir overhead

---

## Plan de Acción

### Prioridad 1: Fix crítico (Throughput)
1. ✅ Reemplazar `database::upsert_pool()` con `db_writer.upsert_pool_full()` en el loop principal
2. ✅ Reemplazar `database::set_pool_activity()` con `db_writer.batch_set_pool_activity()`
3. ✅ Aumentar el rango de bloques procesados por ciclo (de 4 a 20-40 bloques)

### Prioridad 2: Fix importante (Métricas)
4. ✅ Modificar adapters para usar `RpcPool::get_logs_with_recording()` para registrar `eth_getLogs`
5. ✅ Aumentar TTL de Redis a 60 segundos
6. ✅ Agregar verificación de conexión Redis y métricas

### Prioridad 3: Optimización
7. ✅ Revisar y optimizar el procesamiento de bloques para reducir overhead
8. ✅ Agregar métricas adicionales para identificar cuellos de botella

---

## Métricas Esperadas Después de Fixes

- **Throughput**: 2-4 blocks/sec (50-100x mejora)
- **Cache Hit Rate**: 20-40% (después de primer ciclo)
- **RPC Calls**: ~50-100 llamadas por ciclo (incluyendo eth_getLogs)
- **DB Commit Latency**: <10ms promedio (vs ~100ms actual con writes síncronos)
