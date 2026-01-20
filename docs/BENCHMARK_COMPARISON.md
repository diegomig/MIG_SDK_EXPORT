# Comparación de Benchmarks: Histórico vs Actual

**Fecha de comparación**: 18 de Enero, 2026  
**Archivo histórico**: `docs/benchmarks.md`  
**Archivo actual**: `benchmarks/flight_recorder_20260118_014659.jsonl`

---

## 📊 Resumen Ejecutivo

### **Datos Históricos** (benchmarks.md)
- Basado en ejecuciones anteriores del SDK
- Métricas agregadas de múltiples ejecuciones
- Enfoque en rendimiento general del sistema

### **Datos Actuales** (Flight Recorder)
- Captura detallada de eventos en tiempo real
- 230 eventos capturados en ejecución reciente
- Análisis granular de interacciones entre componentes

---

## 🔍 Comparación Detallada

### **1. Discovery Cycles**

#### **Histórico** (benchmarks.md):
- **Latencia promedio**: ~2-3 segundos por ciclo
- **Pools descubiertos**: Variable (depende del estado de la BD)
- **RPC success rate**: Alto (>95%)

#### **Actual** (Flight Recorder):
- **Latencia promedio**: **2317.0ms** (~2.3 segundos) ✅
- **Pools descubiertos**: **0.0** (BD ya poblada)
- **RPC success rate**: **100%** ✅

**Análisis**:
- ✅ Latencia consistente con histórico (2.3s vs 2-3s esperado)
- ✅ RPC success rate mejorado (100% vs >95%)
- ⚠️ 0 pools descubiertos es normal para sistema ya poblado

---

### **2. Graph Updates**

#### **Histórico** (benchmarks.md):
- **Latencia**: Variable según cantidad de pools
- **Pools actualizados**: Depende de discovery
- **Modo**: Incremental o full refresh

#### **Actual** (Flight Recorder):
- **Latencia promedio**: **4029.0ms** (~4 segundos) ✅
- **Pools actualizados**: **78 pools** por actualización
- **Modo**: Incremental (implícito)
- **Hot Pool Manager updated**: **0**

**Análisis**:
- ✅ Latencia razonable para 78 pools (~4 segundos)
- ✅ Actualizaciones incrementales funcionando correctamente
- ⚠️ Hot Pool Manager no se actualizó (puede requerir weights más frescos)

---

### **3. Cache Performance**

#### **Histórico** (benchmarks.md):
- **Cache hit rate**: Variable, típicamente 40-60%
- **Cache events**: No especificado
- **Impacto en RPC**: Reducción significativa de calls

#### **Actual** (Flight Recorder):
- **Cache hit rate**: **50.0%** ✅
- **Cache events**: **156 eventos** (78 hits, 78 misses)
- **Impacto en RPC**: 156 eventos de cache vs 20 RPC calls

**Análisis**:
- ✅ Cache hit rate dentro del rango esperado (50% vs 40-60%)
- ✅ Cache funcionando correctamente
- ✅ Reducción significativa de RPC calls (156 eventos de cache vs 20 calls)

---

### **4. RPC Calls**

#### **Histórico** (benchmarks.md):
- **Success rate**: Alto (>95%)
- **Latencia promedio**: Variable según endpoint
- **Fallos**: Ocasionales

#### **Actual** (Flight Recorder):
- **Success rate**: **100%** ✅
- **Latencia promedio**: **547.2ms** ✅
- **Fallos**: **0** ✅
- **Total calls**: **20 calls**

**Análisis**:
- ✅ Success rate mejorado (100% vs >95%)
- ✅ Latencia razonable para Arbitrum One (~547ms)
- ✅ Sin fallos de RPC

---

### **5. Weight Refresh**

#### **Histórico** (benchmarks.md):
- **Hot refresh**: Cada 30 minutos
- **Warm refresh**: Cada 1 hora
- **Full refresh**: Cada 24 horas
- **Latencia**: Variable según cantidad de pools

#### **Actual** (Flight Recorder):
- **Hot refresh**: **0 eventos** ⏳
- **Warm refresh**: **0 eventos** ⏳
- **Full refresh**: **0 eventos** ⏳

**Razón**:
- El `benchmark_metrics` solo ejecuta discovery cycles y graph updates incrementales
- Los weight refresher tasks solo se ejecutan en `background_discoverer`
- Para ver estos eventos, necesitamos ejecutar `background_discoverer` por más tiempo

**Análisis**:
- ⏳ Weight refresh events no están disponibles en benchmark actual
- ⏳ Requiere ejecución de `background_discoverer` para capturar estos eventos

---

### **6. Hot Pool Manager**

#### **Histórico** (benchmarks.md):
- **Pools en Hot Pool Manager**: Variable (típicamente 50-100)
- **Cache hit rate**: Mejora con Hot Pool Manager poblado
- **Actualización**: Después de graph updates o weight refresh

#### **Actual** (Flight Recorder):
- **Hot Pool Manager updated**: **0** ⏳
- **Pools en Hot Pool Manager**: No especificado
- **Cache hit rate**: **50%** (sin Hot Pool Manager)

**Análisis**:
- ⏳ Hot Pool Manager no se actualizó en esta ejecución
- ⚠️ Puede requerir weights más frescos o más ciclos
- ⚠️ Cache hit rate podría mejorar con Hot Pool Manager poblado

---

## 📊 Comparación de Métricas Clave

| Métrica | Histórico | Actual | Estado |
|---------|-----------|--------|--------|
| **Discovery latency** | 2-3s | 2.3s | ✅ Consistente |
| **Graph update latency** | Variable | 4.0s (78 pools) | ✅ Razonable |
| **RPC success rate** | >95% | 100% | ✅ Mejorado |
| **Cache hit rate** | 40-60% | 50% | ✅ Dentro del rango |
| **RPC avg duration** | Variable | 547ms | ✅ Razonable |
| **Weight refresh events** | Disponible | 0 eventos | ⏳ Requiere `background_discoverer` |
| **Hot Pool Manager** | Variable | 0 actualizado | ⏳ Requiere más ciclos |

---

## 🔄 Interacciones Observadas

### **Histórico** (benchmarks.md):
- Discovery → Graph Update: Funcionando
- Cache → RPC Calls: Reducción significativa
- Weight Refresh → Hot Pool Manager: Funcionando

### **Actual** (Flight Recorder):
- ✅ **Discovery → Graph Update**: Funcionando correctamente
- ✅ **Cache → RPC Calls**: Reducción significativa (156 eventos vs 20 calls)
- ⏳ **Weight Refresh → Hot Pool Manager**: No observado (requiere `background_discoverer`)

**Análisis**:
- ✅ Interacciones básicas funcionando correctamente
- ⏳ Interacciones avanzadas requieren ejecución de `background_discoverer`

---

## ✅ Conclusiones

### **Mejoras Observadas**:

1. ✅ **RPC Success Rate**: Mejorado de >95% a 100%
2. ✅ **Latencia Discovery**: Consistente con histórico (2.3s)
3. ✅ **Cache Performance**: Dentro del rango esperado (50%)
4. ✅ **Graph Updates**: Funcionando correctamente (4.0s para 78 pools)

### **Áreas que Requieren Más Datos**:

1. ⏳ **Weight Refresh**: Requiere ejecución de `background_discoverer`
2. ⏳ **Hot Pool Manager**: Requiere más ciclos o weights más frescos
3. ⏳ **Full Refresh**: Requiere ejecución de 24 horas

### **Recomendaciones**:

1. ✅ **Ejecutar `background_discoverer` por al menos 1 hora** para capturar:
   - Hot refresh inicial (al startup)
   - Hot refresh periódico (cada 30 min)
   - Warm refresh (cada 1 hora)
   - Interacciones con Hot Pool Manager

2. ✅ **Monitorear cache hit rate** con Hot Pool Manager poblado

3. ✅ **Comparar métricas** después de ejecutar `background_discoverer` por más tiempo

---

## 📝 Notas Adicionales

### **Diferencias en Metodología**:

1. **Histórico** (benchmarks.md):
   - Métricas agregadas de múltiples ejecuciones
   - Enfoque en rendimiento general
   - Datos de diferentes momentos en el tiempo

2. **Actual** (Flight Recorder):
   - Captura detallada de eventos en tiempo real
   - Análisis granular de interacciones
   - Datos de una ejecución específica

### **Ventajas del Flight Recorder**:

1. ✅ **Granularidad**: Eventos individuales capturados
2. ✅ **Trazabilidad**: Secuencia temporal de eventos
3. ✅ **Interacciones**: Cómo interactúan los componentes
4. ✅ **Debugging**: Facilita identificación de problemas

### **Limitaciones Actuales**:

1. ⏳ **Weight Refresh**: No disponible en `benchmark_metrics`
2. ⏳ **Hot Pool Manager**: Requiere más ciclos
3. ⏳ **Full Refresh**: Requiere ejecución de 24 horas

---

## 🎯 Próximos Pasos

1. ✅ **Ejecutar `background_discoverer` por al menos 1 hora** para capturar weight refresh events
2. ✅ **Comparar métricas** después de ejecutar `background_discoverer`
3. ✅ **Actualizar benchmarks.md** con nuevos datos del Flight Recorder
4. ✅ **Monitorear cache hit rate** con Hot Pool Manager poblado

---

## ✅ Estado General

**Sistema funcionando correctamente** con métricas consistentes con el histórico. El Flight Recorder proporciona análisis más granular de las interacciones del sistema, pero requiere ejecución de `background_discoverer` para capturar todos los eventos (weight refresh, hot pool manager updates, etc.).
