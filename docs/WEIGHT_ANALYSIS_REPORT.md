# Análisis Profundo: Gestión de Pesos del Grafo en MIG Topology SDK

**Fecha**: 17 de Enero, 2026  
**Analista**: Ingeniero en Sistemas, Especialista en Rust y Arbitrum Tier 1  
**Objetivo**: Identificar la raíz del problema de poblamiento del Hot Pool Manager y proponer soluciones con trade-offs

---

## 1. RESUMEN EJECUTIVO

### Problema Identificado
El Hot Pool Manager no se está poblando correctamente porque la función `load_pool_candidates()` retorna 0 pools cuando busca candidatos con `weight >= 10,000` y `is_active = true`.

### Hallazgo Crítico
**Todos los pools con weights significativos (> 0) están marcados como `is_active = false` en la base de datos.** Esto indica una desconexión entre:
- Los pools procesados por el bot original (con weights calculados pero marcados como inactivos)
- Los pools descubiertos por el SDK actual (marcados como activos pero sin weights calculados aún)

---

## 2. ANÁLISIS DETALLADO DE LA BASE DE DATOS

### 2.1 Distribución de Pesos por Rangos

| Rango de Peso | Total | Activos | Inactivos | Sin Pool Entry | Min | Max | Promedio |
|---------------|-------|---------|-----------|----------------|-----|-----|----------|
| **0** | 26,496 | 17,558 | 8,936 | 2 | 0 | 0 | 0 |
| **< 1** | 3,387 | 0 | 3,387 | 0 | 9.99e-19 | 0.97 | 0.047 |
| **1-10** | 76 | 0 | 76 | 0 | 1.05 | 9.33 | 4.07 |
| **10-100** | 49 | 0 | 49 | 0 | 11.38 | 99.53 | 37.94 |
| **100-1K** | 62 | 0 | 62 | 0 | 100.93 | 999.84 | 395.05 |
| **1K-10K** | 9 | 0 | 9 | 0 | 1,077.82 | 9,864.59 | 4,001.10 |
| **10K-100K** | 13 | 0 | 13 | 0 | 15,358.31 | 99,983.87 | 67,807.37 |
| **100K-1M** | 17 | 0 | 17 | 0 | 106,402.95 | 999,838.68 | 374,334.25 |
| **1M-10M** | 8 | 0 | 8 | 0 | 1,193,611.55 | 9,986,595.83 | 3,432,571.95 |
| **10M-100M** | 6 | 0 | 6 | 0 | 21,262,877.44 | 49,903,917.44 | 33,807,933.35 |
| **100M-1B** | 12 | 0 | 12 | 0 | 121,624,584.87 | 894,258,045.65 | 440,166,354.63 |
| **> 1B** | 17 | 0 | 17 | 0 | 1,168,155,149.36 | 448,465,316,970.63 | 88,566,958,109.11 |

### 2.2 Observaciones Clave

1. **Discrepancia Total**: 
   - **26,496 pools** tienen `weight = 0` (85.5% del total)
   - **17,558 pools activos** tienen `weight = 0` (100% de los activos)
   - **3,656 pools con weight > 0** están TODOS marcados como `is_active = false`

2. **Distribución de Pools Activos vs Inactivos**:
   - **Pools activos**: 17,565 (todos con weight = 0)
   - **Pools inactivos**: 13,077 (3,656 tienen weight > 0)
   - **Pools sin entrada en tabla `pools`**: 2 (con weight = 0)

3. **Top 20 Pools por Weight**:
   - Todos tienen `is_active = false`
   - Todos tienen `is_valid = true`
   - Rango de pesos: $1.17B - $448.47B
   - DEXs principales: UniswapV3 (35 pools), CamelotV3 (18 pools)
   - Última actualización: Noviembre 2025 (datos del bot original)

### 2.3 Distribución por DEX

| DEX | Total Weights | Weight >= 100K | Active Pools | Avg Weight | Max Weight |
|-----|---------------|----------------|--------------|------------|------------|
| **UniswapV2** | 3,074 | 0 | 0 | $22.97 | $64,708.68 |
| **CamelotV2** | 225 | 1 | 0 | $3,152.58 | $705,950.12 |
| **UniswapV3** | 130 | 35 | 0 | $5.4B | $448.47B |
| **SushiSwapV2** | 108 | 5 | 0 | $15,014.84 | $388,645.29 |
| **CamelotV3** | 100 | 18 | 0 | $8.08B | $307.01B |
| **TraderJoe** | 16 | 0 | 0 | $1.41 | $14.25 |
| **KyberSwap** | 3 | 1 | 0 | $298.12M | $894.26M |

**Observación**: Los pools V3 (UniswapV3, CamelotV3) tienen weights extremadamente altos (billones), mientras que los pools V2 tienen weights mucho menores (miles a millones).

---

## 3. ANÁLISIS DEL CÓDIGO DEL SDK

### 3.1 Fórmulas de Cálculo de Weight

#### Uniswap V2 (Línea 1134)
```rust
weight = reserve0_f * price0 + reserve1_f * price1
```
Donde:
- `reserve0_f = reserve0 / 10^decimals0`
- `reserve1_f = reserve1 / 10^decimals1`

**Veredicto**: ✅ **Correcto** - Suma directa de valores USD de ambas reservas.

#### Uniswap V3 (Líneas 1200-1258)
```rust
amount0_raw = liquidity * Q96 / sqrt_price_x96
amount1_raw = liquidity * sqrt_price_x96 / Q96
amount0 = amount0_raw / 10^decimals0
amount1 = amount1_raw / 10^decimals1
weight = amount0 * price0 + amount1 * price1
```

**Veredicto**: ⚠️ **Discrepancia con documentación** - La documentación dice `weight = sqrt(reserve0 * price0 * reserve1 * price1)`, pero el código usa suma directa. Sin embargo, la fórmula implementada es **matemáticamente correcta** para V3.

### 3.2 Almacenamiento en Base de Datos

**Función**: `database::upsert_graph_weight()` (Línea 789)
```rust
INSERT INTO mig_topology.graph_weights (pool_address, weight, last_computed_block, updated_at)
VALUES ($1, $2, $3, $4)
ON CONFLICT(pool_address) DO UPDATE SET weight=excluded.weight, ...
```

**Observación**: El SDK almacena `weight` directamente como `f64` (liquidez USD calculada), sin normalización.

### 3.3 Query de Carga de Candidatos

**Función**: `database::load_pool_candidates()` (Línea 838)
```sql
SELECT gw.pool_address, gw.weight, gw.updated_at
FROM mig_topology.graph_weights gw
INNER JOIN mig_topology.pools p ON p.address = gw.pool_address
WHERE p.is_active = true
  AND gw.weight >= $1
  AND (gw.updated_at IS NULL OR gw.updated_at >= $2)
ORDER BY gw.weight DESC
LIMIT $3
```

**Problema Identificado**: 
- La query usa `INNER JOIN` con `p.is_active = true`
- **Todos los pools con weight > 0 tienen `is_active = false`**
- Resultado: 0 candidatos encontrados

---

## 4. ANÁLISIS DE LA RAÍZ DEL PROBLEMA

### 4.1 Causa Raíz

**Desconexión Temporal y de Estado entre Bot Original y SDK**:

1. **Bot Original** (Noviembre 2025):
   - Procesó y calculó weights para ~3,656 pools
   - Marcó estos pools como `is_active = false` (posiblemente porque dejaron de tener actividad o fueron deprecados)
   - Los weights quedaron almacenados en `graph_weights` con valores válidos

2. **SDK Actual** (Enero 2026):
   - Descubre nuevos pools y los marca como `is_active = true`
   - Estos pools nuevos aún no tienen weights calculados (weight = 0)
   - La función `load_pool_candidates()` busca pools activos con weights altos
   - **Resultado**: No encuentra ningún pool porque los activos tienen weight = 0 y los con weight > 0 están inactivos

### 4.2 Evidencia de la Desconexión

1. **Timestamps**: Los top pools tienen `updated_at = 2025-11-06` (2 meses atrás)
2. **Estado**: Todos los pools con weight > 0 tienen `is_active = false`
3. **Distribución**: 100% de pools activos tienen weight = 0
4. **Volumen**: Hay 60 pools con weight >= $100K, pero todos están inactivos

### 4.3 Posibles Razones para `is_active = false`

1. **Pools deprecados**: Los pools pueden haber sido deprecados por el bot original
2. **Falta de liquidez actual**: Los pools pueden haber perdido liquidez y el bot los marcó como inactivos
3. **Migración de datos**: Durante la migración del schema `arbitrage` → `mig_topology`, los flags pueden haberse reseteado
4. **Lógica de activación**: El bot original puede tener una lógica diferente para marcar pools como activos

---

## 5. ANÁLISIS DE SOLUCIONES POSIBLES

### Solución 1: Ignorar Flag `is_active` para Candidatos

**Descripción**: Modificar `load_pool_candidates()` para buscar pools con weights altos independientemente del estado `is_active`.

**Implementación**:
```sql
SELECT gw.pool_address, gw.weight, gw.updated_at
FROM mig_topology.graph_weights gw
LEFT JOIN mig_topology.pools p ON p.address = gw.pool_address
WHERE gw.weight >= $1
  AND (gw.updated_at IS NULL OR gw.updated_at >= $2)
  AND (p.address IS NULL OR p.is_valid = true)  -- Solo validar que sean válidos si existen
ORDER BY gw.weight DESC
LIMIT $3
```

**Ventajas**:
- ✅ Solución inmediata y simple
- ✅ Aprovecha los weights ya calculados del bot original
- ✅ No requiere recalcular weights para pools existentes
- ✅ Permite poblar Hot Pool Manager inmediatamente

**Desventajas**:
- ⚠️ Puede incluir pools que realmente están deprecados/inactivos
- ⚠️ No distingue entre pools históricos válidos y pools actualmente activos
- ⚠️ Puede poblar Hot Pool Manager con pools que ya no tienen liquidez

**Trade-off**: **Rapidez vs Precisión** - Solución rápida pero puede incluir datos stale.

---

### Solución 2: Recalcular Weights para Pools Activos

**Descripción**: Ejecutar `calculate_and_update_all_weights()` para todos los pools activos antes de poblar Hot Pool Manager.

**Implementación**:
1. Ejecutar full refresh de weights para todos los pools activos
2. Esperar a que se completen los cálculos
3. Luego ejecutar `load_pool_candidates()` con `is_active = true`

**Ventajas**:
- ✅ Usa solo pools actualmente activos
- ✅ Weights frescos y actualizados
- ✅ Alineado con la arquitectura del SDK

**Desventajas**:
- ⚠️ Requiere tiempo significativo (~40-60s para 20k+ pools)
- ⚠️ Requiere múltiples llamadas RPC (costoso)
- ⚠️ No aprovecha los weights ya calculados del bot original
- ⚠️ Puede resultar en weights = 0 si no hay precios disponibles

**Trade-off**: **Precisión vs Performance** - Solución precisa pero costosa en tiempo y recursos.

---

### Solución 3: Híbrida - Usar Weights Existentes con Validación

**Descripción**: Usar weights del bot original pero validar que los pools sigan siendo válidos antes de agregarlos al Hot Pool Manager.

**Implementación**:
1. Cargar candidatos sin filtrar por `is_active`
2. Validar cada pool:
   - Verificar que el pool existe on-chain
   - Verificar que tiene liquidez actual
   - Verificar que el weight sigue siendo razonable
3. Solo agregar pools que pasen la validación

**Ventajas**:
- ✅ Aprovecha weights existentes (rápido)
- ✅ Valida pools antes de agregarlos (preciso)
- ✅ Mejor balance entre rapidez y precisión
- ✅ Resiliente a datos stale

**Desventajas**:
- ⚠️ Requiere validación on-chain (llamadas RPC adicionales)
- ⚠️ Más complejo de implementar
- ⚠️ Puede filtrar muchos pools si están realmente deprecados

**Trade-off**: **Complejidad vs Calidad** - Solución balanceada pero más compleja.

---

### Solución 4: Marcar Pools como Activos Basado en Weight

**Descripción**: Crear una función que marque pools como activos si tienen weight significativo y son válidos.

**Implementación**:
1. Identificar pools con weight >= threshold que están marcados como inactivos
2. Validar que siguen siendo válidos on-chain
3. Actualizar `is_active = true` para pools válidos
4. Luego usar la query original con `is_active = true`

**Ventajas**:
- ✅ Corrige el estado de la base de datos
- ✅ Alinea datos históricos con estado actual
- ✅ Permite usar la query original sin modificaciones

**Desventajas**:
- ⚠️ Modifica datos históricos (puede ser problemático)
- ⚠️ Requiere validación on-chain para todos los pools
- ⚠️ Puede marcar pools como activos incorrectamente si los weights son stale

**Trade-off**: **Corrección de Datos vs Integridad Histórica** - Solución que corrige el problema pero modifica datos históricos.

---

### Solución 5: Usar Threshold Dinámico Basado en Distribución

**Descripción**: Calcular un threshold dinámico basado en la distribución real de weights en la BD, en lugar de usar un valor fijo.

**Implementación**:
1. Calcular percentiles de weights (P25, P50, P75, P90)
2. Usar un percentil alto (P90 o P95) como threshold
3. Cargar candidatos usando este threshold dinámico
4. No filtrar por `is_active` inicialmente

**Ventajas**:
- ✅ Se adapta a la distribución real de datos
- ✅ Encuentra pools relevantes independientemente del estado
- ✅ Más robusto a cambios en la distribución de weights

**Desventajas**:
- ⚠️ Puede incluir pools con weights muy bajos si la distribución es sesgada
- ⚠️ Requiere cálculo adicional de percentiles
- ⚠️ No resuelve el problema de `is_active = false`

**Trade-off**: **Adaptabilidad vs Control** - Solución adaptativa pero menos controlada.

---

## 6. RECOMENDACIÓN FINAL

### Solución Recomendada: **Solución 3 (Híbrida)** con elementos de Solución 1

**Justificación**:
1. **Inmediatez**: Permite poblar Hot Pool Manager inmediatamente usando datos existentes
2. **Precisión**: Valida pools antes de agregarlos, evitando datos stale
3. **Eficiencia**: Aprovecha weights ya calculados sin requerir full refresh
4. **Resiliencia**: Funciona incluso si algunos pools están deprecados

### Implementación Propuesta

**Paso 1**: Modificar `load_pool_candidates()` para no requerir `is_active = true`:
```sql
SELECT gw.pool_address, gw.weight, gw.updated_at
FROM mig_topology.graph_weights gw
LEFT JOIN mig_topology.pools p ON p.address = gw.pool_address
WHERE gw.weight >= $1
  AND (gw.updated_at IS NULL OR gw.updated_at >= $2)
  AND (p.address IS NULL OR p.is_valid = true OR p.is_active = true)
ORDER BY gw.weight DESC
LIMIT $3
```

**Paso 2**: Agregar validación en `populate_hot_pool_manager_from_db()`:
- Después de cargar pools, validar que tengan estado válido on-chain
- Solo agregar pools que pasen la validación
- Loggear pools filtrados para análisis

**Paso 3**: Ajustar threshold inicial:
- Usar `min_weight = 10_000.0` (ya implementado)
- Considerar threshold dinámico basado en P90 si es necesario

### Plan de Implementación

1. **Corto Plazo** (Inmediato):
   - Modificar query para incluir pools inactivos con weights válidos
   - Agregar validación básica on-chain antes de agregar al Hot Pool Manager
   - Ejecutar benchmark para verificar funcionamiento

2. **Mediano Plazo** (Siguiente sprint):
   - Implementar validación más robusta (verificar liquidez actual)
   - Agregar métricas de validación (cuántos pools se filtran y por qué)
   - Considerar threshold dinámico basado en percentiles

3. **Largo Plazo** (Futuro):
   - Sincronizar flags `is_active` con weights calculados
   - Implementar proceso de limpieza para pools deprecados
   - Considerar migración de datos históricos si es necesario

---

## 7. MÉTRICAS DE ÉXITO

### Métricas Inmediatas
- ✅ Hot Pool Manager poblado con > 0 pools después del primer ciclo
- ✅ Cache hit rate > 0% después de algunos ciclos
- ✅ Pools agregados al Hot Pool Manager tienen weights válidos

### Métricas a Mediano Plazo
- ✅ Cache hit rate > 80% después de 5+ ciclos
- ✅ Hot Pool Manager mantiene 50+ pools válidos
- ✅ Tasa de validación exitosa > 70% (pools que pasan validación on-chain)

### Métricas a Largo Plazo
- ✅ Sincronización entre `is_active` y weights calculados
- ✅ Reducción de pools deprecados en la BD
- ✅ Mejora en la calidad de datos históricos

---

## 8. CONCLUSIÓN

El problema identificado es una **desconexión entre el estado de pools (`is_active`) y los weights calculados**. Los pools con weights significativos están marcados como inactivos, mientras que los pools activos aún no tienen weights calculados.

La solución recomendada es un **enfoque híbrido** que:
1. Aprovecha los weights existentes del bot original
2. Valida pools antes de agregarlos al Hot Pool Manager
3. Permite poblamiento inmediato sin requerir full refresh

Esta solución balancea **rapidez, precisión y eficiencia**, permitiendo que el SDK funcione correctamente mientras se resuelven los problemas de sincronización de datos a largo plazo.

---

**Próximos Pasos**:
1. ✅ **COMPLETADO**: Modificar query para incluir pools inactivos con weights válidos
2. Ejecutar benchmark para verificar funcionamiento (esperado: 73 candidatos encontrados)
3. Monitorear métricas de validación y cache hit rate
4. Planificar sincronización de datos históricos si es necesario

---

## 9. VALIDACIÓN DE LA SOLUCIÓN

### Query Modificada (Implementada)
```sql
SELECT gw.pool_address, gw.weight, gw.updated_at
FROM mig_topology.graph_weights gw
LEFT JOIN mig_topology.pools p ON p.address = gw.pool_address
WHERE gw.weight >= $1
  AND (gw.updated_at IS NULL OR gw.updated_at >= $2)
  AND (p.address IS NULL OR p.is_valid = true OR p.is_active = true)
ORDER BY gw.weight DESC
LIMIT $3
```

### Resultados de Prueba
- **Candidatos encontrados con weight >= 10K**: **73 pools** ✅
- **Candidatos encontrados con weight >= 100K**: **60 pools** ✅
- **Mejora**: De 0 candidatos a 73 candidatos disponibles

### Próxima Validación
Ejecutar benchmark completo para verificar:
1. Hot Pool Manager se pobla correctamente
2. Cache hit rate mejora significativamente
3. Pools agregados tienen estados válidos on-chain
