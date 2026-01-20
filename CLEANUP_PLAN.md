# Plan de Limpieza: Eliminación de Referencias a Trading/Arbitrage

**Fecha**: Enero 2025  
**Objetivo**: Eliminar TODAS las referencias a trading/arbitrage del código activo

---

## 🔴 ELIMINACIÓN INMEDIATA (Dead Code Confirmado)

### 1. `postgres_async_writer.rs` - OpportunityData y BatchOpportunities

**Código a Eliminar**:
- Struct `OpportunityData` (líneas 65-81)
- Enum variant `BatchOpportunities` (líneas 47-49)
- Match arm en `flush_batch()` (líneas 240-242)
- Función `batch_insert_opportunities()` (líneas 479-516)
- Método `batch_opportunities()` (líneas 474-476)
- Vector `opportunities` en `flush_batch()` (línea 215)

**Justificación**: Código activo pero NO usado (verificado: no hay llamadas a `batch_opportunities()`)

---

### 2. `database.rs` - Tablas de Trading

**Código a Eliminar**:
- Creación tabla `arbitrage.opportunities` (líneas 432-453)
- Creación tabla `arbitrage.opportunity_diagnostics` (líneas 456-478)
- Creación tabla `arbitrage.executions` (líneas 491-507)
- Índices relacionados:
  - `idx_opportunities_status` (línea 539)
  - `idx_opportunities_detected_at` (línea 543)
  - `idx_opportunities_block_number` (línea 545)
  - `idx_executions_op_id` (línea 546)
  - `idx_opdiag_block` (línea 549)
  - `idx_opdiag_status` (línea 551)

**Justificación**: Tablas se crean pero NO se usan (solo código comentado las usaba)

---

### 3. `database.rs` - Columnas `profit_*` en pool_statistics

**Problema**: Columnas `avg_profit_usd`, `last_profit_usd`, `profit_sample_count` se crean pero se bindean con `None`.

**Código a Eliminar/Modificar**:
- Columnas `avg_profit_usd`, `profit_sample_count`, `last_profit_usd` del CREATE TABLE (líneas 391-393)
- Índice `idx_pool_statistics_profit` (líneas 409-414)
- References en INSERT/UPDATE de `apply_pool_stats_update()` (líneas 1403-1405, 1413-1415, 1434-1436)

**Justificación**: Columnas no se usan (siempre se bindean con None)

---

### 4. `redis_manager.rs` - Queue `hot_opportunities`

**Código a Eliminar**:
- Función `get_queue_length()` completa (líneas 105-116)
- Referencia en `clear_cache()` (línea 359)
- Métricas relacionadas en `metrics.rs` (líneas 1019-1020, 1758-1759)

**Justificación**: Código legacy, queue no se usa activamente

---

### 5. `redis_manager.rs` - Config Fields No Usados

**Código a Eliminar**:
- Campo `execution_tracker_ttl` (línea 34)
- Campo `opportunity_ttl` (línea 32)
- Default values (líneas 43-44, 56-57)

**Justificación**: Marcados como "Unused but kept for compatibility"

---

### 6. `settings.rs` - Funciones Legacy No Usadas

**Código a Eliminar**:
- `default_max_slippage_during_execution_bps()` (línea 726)
- `default_max_gas_price_during_execution_gwei()` (línea 729)
- Funciones relacionadas a flash loans (líneas 717-740):
  - `default_max_flash_loan_per_trade_usd()`
  - `default_max_flash_loan_utilization()`
  - `default_min_pool_liquidity_multiplier()`
  - `default_min_profit_drop_percentage()`
  - `default_max_flash_loan_failures_per_hour()`
  - `default_flash_loan_cooldown_seconds()`

**Justificación**: Funciones definidas pero NO se usan (no hay structs que las usen)

---

### 7. `settings.rs` - Structs Legacy No Usados

**Verificar si se usan**:
- `Sizing` struct (líneas 1220-1251) - Parece trading-related
- `MVP` struct (líneas 1422-1506) - "Minimal Viable Product" pero contiene campos trading
- `Warming` struct - Necesito verificar

**Si NO se usan**: Eliminar completamente

---

### 8. `settings.rs` - Campos en Structs No Usados

**Campos a Verificar/Eliminar**:
- `multi_arbitrage_address` (línea 244) - Verificar uso
- `enable_execution` (línea 1053) - Verificar contexto (puede ser WebSocket execution)
- `revalidate_reserves_before_execution` (línea 1439) - Verificar uso
- `min_trade_size_usd`, `max_trade_size_usd`, `max_trade_liquidity_pct` (líneas 801-804) - Verificar uso
- Campos en `MVPAuto`: `min_profit_to_gas_ratio`, `max_exec_per_day` (líneas 1481, 1483)

---

## 🟡 CAMBIOS SIMPLES (Sin Riesgo)

### 9. Database URL Default

**Cambiar**: `src/settings.rs` línea 639
```rust
// Antes
"postgresql://user:pass@127.0.0.1:5432/arbitrage_bot".to_string()

// Después
"postgresql://user:pass@127.0.0.1:5432/mig_topology".to_string()
```

---

### 10. Schema Name `arbitrage`

**Decisión**: MANTENER pero documentar como legacy

**Razón**: 
- Cambiar requeriría migration script
- Puede romper bases de datos existentes
- Muchas queries afectadas (cientos)

**Acción**: Documentar claramente en comentarios que es legacy schema name.

---

## 📋 RESUMEN DE ELIMINACIONES

### Código Dead (Eliminar Sin Dudas)

1. ✅ `OpportunityData` struct + funciones relacionadas
2. ✅ Tablas `opportunities`, `executions`, `opportunity_diagnostics` + índices
3. ✅ Columnas `profit_*` en `pool_statistics` + índice
4. ✅ Redis queue `hot_opportunities` + función `get_queue_length()`
5. ✅ Redis config `execution_tracker_ttl`, `opportunity_ttl`
6. ✅ Funciones flash loan defaults (si no se usan)
7. ✅ Structs `Sizing`, `MVP`, `Warming` (si no se usan)

### Cambios Simples

8. ✅ Database URL default: `arbitrage_bot` → `mig_topology`
9. ⚠️ Schema name `arbitrage`: Documentar como legacy (no cambiar)

### Verificación Pendiente

10. ⏳ `multi_arbitrage_address` - Verificar uso
11. ⏳ `enable_execution` - Verificar contexto
12. ⏳ `revalidate_reserves_before_execution` - Verificar uso
13. ⏳ `min_trade_size_usd`, etc. - Verificar contexto
14. ⏳ `Sizing`, `MVP`, `Warming` structs - Verificar uso

---

**Estado**: Listo para empezar eliminaciones confirmadas.
