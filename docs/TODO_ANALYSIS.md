# Análisis de TODOs: Qué Implementar vs Qué Ya Está Resuelto

**Fecha**: 17 de Enero, 2026  
**Análisis**: Revisión detallada de cada TODO vs código existente

---

## ✅ Ya Resuelto (Versión Mejor Disponible)

### 1. **State Fetching Real en `pools.rs`** ✅ OBSOLETO

**TODO**: `src/pools.rs` líneas 221, 239
```rust
// TODO: Implement real state fetching logic
```

**Análisis**:
- ❌ Los métodos `fetch_state()` en `BalancerWeightedPool` y `CurveStableSwapPool` están vacíos
- ✅ **PERO** existe `UnifiedStateFetcher` que SÍ implementa fetching completo para Balancer y Curve
- ✅ Los adapters (`balancer_v2.rs`, `balancer_v3.rs`, `curve.rs`) implementan `fetch_pool_state()` correctamente
- ✅ `GraphService::fetch_pool_states()` usa `UnifiedStateFetcher` internamente

**Conclusión**: 
- **NO implementar** - Los métodos en `pools.rs` son obsoletos
- **Acción recomendada**: Eliminar los métodos `fetch_state()` de `BalancerWeightedPool` y `CurveStableSwapPool` o marcarlos como deprecated
- El código real está en `UnifiedStateFetcher` y los adapters

---

### 6. **CoinGecko para Precios** ✅ YA IMPLEMENTADO

**TODO**: `docs/WEIGHT_REFRESHER_IMPLEMENTATION.md`

**Análisis**:
- ✅ Ya existe `coingecko_price_updater.rs` con implementación completa
- ✅ Ya existe `external_price_updater.rs` con múltiples fuentes (Binance, Pyth, DefiLlama, CoinGecko)
- ✅ Ya existe `background_price_updater.rs` que usa `SharedPriceCache` y actualiza precios periódicamente
- ✅ El sistema ya tiene fallback a múltiples fuentes externas

**Conclusión**:
- **NO implementar** - Ya está implementado y funcionando
- **Nota**: El TODO en la documentación está desactualizado. CoinGecko ya está integrado en el sistema de precios externos

---

## 🟡 Conviene Implementar (Mejoras Importantes)

### 2. **Cargar Blacklisted Tokens desde Settings** 🟡 IMPLEMENTAR

**TODO**: `src/background_pool_validator.rs` línea 375
```rust
// TODO: Cargar desde settings.validator.blacklisted_tokens
```

**Análisis**:
- ✅ Ya existe `settings.validator.blacklisted_tokens: Vec<String>` en `settings.rs`
- ❌ El código usa una constante vacía `const BLACKLISTED_TOKENS: &[Address] = &[];`
- ✅ La infraestructura está lista, solo falta conectar

**Beneficio**: 
- Mejora configurabilidad
- Permite blacklistear tokens sin recompilar

**Complejidad**: Baja (solo parsear strings a Address)

**Conclusión**: **✅ IMPLEMENTAR** - Es una mejora simple y útil

---

### 3. **Convertir a ETH usando Price Feed** 🟡 MEJORAR

**TODO**: `src/pool_filters.rs` línea 341
```rust
// TODO: Convert to ETH using price feed
```

**Análisis**:
- ✅ Ya existe `estimate_eth_value()` que usa `global_prices`
- ⚠️ Usa precio ETH hardcodeado (`3000.0`)
- ✅ Ya existe `PriceFeed` que puede obtener precio de ETH/WETH
- ⚠️ No está conectado al `PriceFeed` real

**Beneficio**:
- Mejora precisión de filtros de liquidez
- Usa precio ETH real en lugar de hardcodeado

**Complejidad**: Media (necesita acceso a `PriceFeed`)

**Conclusión**: **✅ MEJORAR** - Ya hay versión básica, pero se puede mejorar usando `PriceFeed` real

---

### 4. **Trigger Re-sync Automático** 🟡 IMPLEMENTAR

**TODO**: `src/event_indexer.rs` línea 144
```rust
// TODO: Trigger re-sync automatically
```

**Análisis**:
- ✅ El código detecta gaps correctamente
- ❌ Solo loggea el error pero no ejecuta re-sync
- ✅ Existe infraestructura de re-sync en el código

**Beneficio**:
- Mejora resiliencia automática
- Reduce necesidad de intervención manual

**Complejidad**: Media (requiere coordinar con orchestrator)

**Conclusión**: **✅ IMPLEMENTAR** - Mejora importante de resiliencia

---

## 🟢 Baja Prioridad (Solo si se Necesita)

### 5. **Redis Pub/Sub para Block Stream** 🟢 OPCIONAL

**TODO**: `src/block_stream.rs` línea 109
```rust
// TODO: Add publish method to RedisManager if needed
```

**Análisis**:
- ✅ Ya existe infraestructura de broadcast in-process
- ⚠️ Redis pub/sub solo necesario para multi-process coordination
- ✅ El código funciona sin Redis pub/sub

**Beneficio**:
- Útil solo si se necesita multi-process coordination
- No crítico para funcionamiento single-process

**Complejidad**: Media (agregar método a RedisManager)

**Conclusión**: **⚠️ SOLO SI SE NECESITA** - No crítico, solo útil para arquitectura multi-process

---

## 🔵 Optimizaciones Futuras (No Urgentes)

### 7. **Coordinación con Discovery mediante Channels** 🔵 FUTURO

**Estado**: No implementado

**Análisis**:
- Mejora latencia para pools nuevos importantes
- Requiere arquitectura de eventos/channels

**Conclusión**: **⏸️ FUTURO** - Mejora pero no crítica, puede implementarse después

---

### 8. **Frecuencia Adaptativa** 🔵 FUTURO

**Estado**: No implementado

**Análisis**:
- Optimización avanzada
- Requiere métricas de staleness y algoritmo adaptativo

**Conclusión**: **⏸️ FUTURO** - Optimización avanzada, no urgente

---

## 📊 Resumen Ejecutivo

| TODO | Estado | Acción | Prioridad |
|------|--------|--------|-----------|
| **State fetching real** | ✅ Ya resuelto | Eliminar TODO (obsoleto) | - |
| **Blacklisted tokens** | 🟡 Implementar | Conectar con settings | Media |
| **Convert to ETH** | 🟡 Mejorar | Usar PriceFeed real | Media |
| **Re-sync automático** | 🟡 Implementar | Trigger automático | Media |
| **Redis pub/sub** | 🟢 Opcional | Solo si multi-process | Baja |
| **CoinGecko precios** | ✅ Ya resuelto | Actualizar docs | - |
| **Coordinación discovery** | 🔵 Futuro | Optimización futura | Baja |
| **Frecuencia adaptativa** | 🔵 Futuro | Optimización avanzada | Baja |

---

## 🎯 Recomendación Final

### **Implementar Ahora** (3 items):
1. ✅ **Blacklisted tokens desde settings** - Simple y útil
2. ✅ **Mejorar conversión a ETH** - Usar PriceFeed real
3. ✅ **Re-sync automático** - Mejora resiliencia

### **Eliminar/Marcar como Obsoleto** (2 items):
1. ❌ **State fetching en pools.rs** - Ya resuelto en UnifiedStateFetcher
2. ❌ **CoinGecko TODO en docs** - Ya implementado

### **Dejar para Futuro** (3 items):
1. ⏸️ Redis pub/sub (solo si se necesita)
2. ⏸️ Coordinación discovery (optimización futura)
3. ⏸️ Frecuencia adaptativa (optimización avanzada)

---

## 📝 Notas Adicionales

- Los TODOs obsoletos deberían eliminarse o marcarse como deprecated para evitar confusión
- Las optimizaciones futuras están bien documentadas y pueden implementarse cuando sea necesario
- Los 3 items recomendados para implementar son mejoras incrementales que no requieren cambios arquitectónicos grandes
