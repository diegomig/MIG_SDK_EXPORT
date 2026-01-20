# TODOs Pendientes

**Fecha**: 17 de Enero, 2026  
**Estado**: Organizado por Prioridad

---

## 🔴 Alta Prioridad (Funcionalidad Crítica)

### 1. **Implementar State Fetching Real en `pools.rs`** ⚠️
**Ubicación**: `src/pools.rs` (líneas 221, 239)

**Descripción**: 
- Actualmente hay TODOs para implementar lógica real de fetching de estado
- Probablemente son métodos stub que necesitan implementación

**Impacto**: Puede afectar funcionalidad core del SDK

**Acción**: Revisar qué métodos necesitan implementación y completarlos

---

## 🟡 Media Prioridad (Mejoras Importantes)

### 2. **Cargar Blacklisted Tokens desde Settings** ⚠️
**Ubicación**: `src/background_pool_validator.rs` (línea 375)

**Descripción**: 
```rust
// TODO: Cargar desde settings.validator.blacklisted_tokens
```

**Impacto**: Mejora configurabilidad del validador

**Acción**: Implementar carga de tokens blacklisted desde configuración

---

### 3. **Convertir a ETH usando Price Feed** ⚠️
**Ubicación**: `src/pool_filters.rs` (línea 341)

**Descripción**: 
```rust
// TODO: Convert to ETH using price feed
```

**Impacto**: Mejora precisión de filtros de pools

**Acción**: Implementar conversión a ETH usando PriceFeed

---

### 4. **Trigger Re-sync Automático en Event Indexer** ⚠️
**Ubicación**: `src/event_indexer.rs` (línea 144)

**Descripción**: 
```rust
// TODO: Trigger re-sync automatically
```

**Impacto**: Mejora resiliencia del indexer

**Acción**: Implementar trigger automático de re-sync cuando sea necesario

---

## 🟢 Baja Prioridad (Optimizaciones Futuras)

### 5. **Redis Pub/Sub para Block Stream** ⚠️
**Ubicación**: `src/block_stream.rs` (línea 109)

**Descripción**: 
```rust
// TODO: Add publish method to RedisManager if needed
```

**Impacto**: Mejora distribución de eventos de bloques

**Acción**: Agregar método de publicación a RedisManager si se necesita

---

## 📋 Optimizaciones Documentadas (Weight Refresher)

### 6. **Integrar CoinGecko para Precios** ⚠️
**Ubicación**: `docs/WEIGHT_REFRESHER_IMPLEMENTATION.md`

**Estado**: No implementado (requiere integración con `PriceFeed`)

**Beneficio**: 
- Reducción de costos RPC (~80% ahorro en llamadas de precios)
- Más rápido que Chainlink oracles
- Cubre más tokens

**Complejidad**: Media (requiere modificar `calculate_liquidity_usd_with_cache()`)

**Próximo paso**: Integrar CoinGecko directamente en `calculate_liquidity_usd_with_cache()`

---

### 7. **Coordinación con Discovery mediante Channels** ⚠️
**Ubicación**: `docs/WEIGHT_REFRESHER_IMPLEMENTATION.md`

**Estado**: No implementado

**Beneficio**: 
- Actualización inmediata cuando discovery encuentra pools grandes
- Mejor latencia para pools nuevos importantes

**Complejidad**: Media-Alta (requiere channels y eventos)

**Próximo paso**: Implementar sistema de eventos para notificar discovery de pools grandes

---

### 8. **Frecuencia Adaptativa Basada en Staleness** ⚠️
**Ubicación**: `docs/WEIGHT_REFRESHER_IMPLEMENTATION.md`

**Estado**: No implementado

**Beneficio**: 
- Ajuste automático de frecuencia basado en cambios reales
- Optimización de costos RPC

**Complejidad**: Alta (requiere métricas de staleness y algoritmo adaptativo)

**Próximo paso**: Implementar métricas de staleness y algoritmo adaptativo

---

## 🔵 Mejoras de Observabilidad (Fase 3)

### 9. **Detector de Weights Extremos** ⚠️
**Ubicación**: `docs/IMPLEMENTATION_SUMMARY.md`

**Estado**: No implementado (no urgente)

**Descripción**: Detectar pools con weights anómalos (> $1B) y re-verificar on-chain

**Impacto**: Mejora calidad de datos

**Prioridad**: Baja (filtro existente es suficiente por ahora)

---

### 10. **Health Check de Weights** ⚠️
**Ubicación**: `docs/IMPLEMENTATION_SUMMARY.md`

**Estado**: No implementado

**Descripción**: Health check después de `calculate_and_update_all_weights()`

**Impacto**: Mejora observabilidad

**Prioridad**: Baja (mejora futura)

---

### 11. **Dashboard de Métricas** ⚠️
**Ubicación**: `docs/IMPLEMENTATION_SUMMARY.md`

**Estado**: No implementado

**Descripción**: Dashboard para visualizar métricas de pools, weights, etc.

**Impacto**: Mejora observabilidad y debugging

**Prioridad**: Baja (mejora futura)

---

## 📊 Resumen por Prioridad

| Prioridad | Cantidad | TODOs |
|-----------|----------|-------|
| 🔴 **Alta** | 1 | State fetching real |
| 🟡 **Media** | 3 | Blacklisted tokens, Convert to ETH, Re-sync automático |
| 🟢 **Baja** | 1 | Redis pub/sub |
| 📋 **Optimizaciones** | 3 | CoinGecko, Coordinación discovery, Frecuencia adaptativa |
| 🔵 **Observabilidad** | 3 | Detector weights, Health check, Dashboard |

**Total**: 11 TODOs pendientes

---

## 🎯 Recomendación de Orden

1. **Primero**: Revisar y completar state fetching real (`pools.rs`) - puede ser crítico
2. **Segundo**: Implementar mejoras de configurabilidad (blacklisted tokens, convert to ETH)
3. **Tercero**: Optimizaciones de costo (CoinGecko para precios)
4. **Cuarto**: Mejoras de coordinación (discovery channels, re-sync automático)
5. **Quinto**: Optimizaciones avanzadas (frecuencia adaptativa, observabilidad)

---

## 📝 Notas

- Los TODOs marcados como "optimizaciones futuras" son opcionales y pueden implementarse según necesidad
- Los TODOs de observabilidad son mejoras pero no críticos para funcionalidad
- Priorizar según impacto en producción y complejidad de implementación
