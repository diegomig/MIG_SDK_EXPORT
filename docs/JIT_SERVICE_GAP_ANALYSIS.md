# Análisis de Brechas: Servicio JIT según Propuesta de Grant

**Fecha**: 18 de Enero, 2026  
**Referencia**: `grants/arbitrum/Core_Proposal/PROJECT_PROPOSAL.md`  
**Estado Actual**: Basado en `benchmarks/flight_recorder_20260118_014659.jsonl`

---

## 🎯 Objetivos del Servicio JIT (Según Propuesta)

### **Milestone 1 Targets**:
1. ✅ **Cache hit rate**: ≥80% (actualmente: 50%)
2. ✅ **JIT state fetch latency**: ≤10ms (local node), ≤100ms (remote RPC)
3. ✅ **RPC calls per block**: ≤30 (actualmente: 20 calls en benchmark)
4. ✅ **End-to-end latency**: ≤200ms (discovery → graph update)

---

## 📊 Estado Actual vs Objetivos

| Métrica | Objetivo | Actual | Gap | Estado |
|---------|----------|--------|-----|--------|
| **Cache hit rate** | ≥80% | 50% | -30% | ⚠️ **FALTA 30%** |
| **JIT latency (local)** | ≤10ms | N/A | N/A | ⏳ **NO MEDIDO** |
| **JIT latency (remote)** | ≤100ms | ~547ms | +447ms | ❌ **FALTA OPTIMIZACIÓN** |
| **RPC calls/block** | ≤30 | 20 | ✅ | ✅ **CUMPLIDO** |
| **End-to-end latency** | ≤200ms | ~6,300ms | +6,100ms | ❌ **FALTA OPTIMIZACIÓN** |

---

## 🔍 Análisis Detallado de Brechas

### **1. Cache Hit Rate: 50% → 80%** ⚠️

**Gap**: -30 puntos porcentuales

**Causas Identificadas**:
1. ⚠️ **Hot Pool Manager no poblado**: 0 pools en Hot Pool Manager
2. ⚠️ **TTL no diferenciado**: No hay TTL diferenciado para touched/untouched pools
3. ⚠️ **Cache invalidation agresiva**: Puede estar invalidando cache innecesariamente
4. ⚠️ **Fuzzy block matching**: Puede no estar implementado (5-block tolerance)

**Lo que falta implementar** (según propuesta):

1. **Merkle Tree-Based Cache Invalidation** ❌
   - Estado actual: Cache invalidation basada en bloques
   - Falta: State hash calculation (sqrt_price_x96, liquidity, tick para V3; reserves para V2)
   - Falta: Cache invalidation solo cuando cambia el state hash (no block-based)

2. **TTL Differentiation Strategy** ❌
   - Estado actual: TTL uniforme (probablemente)
   - Falta: 30s TTL para touched pools (recientes Swap/Mint/Burn events)
   - Falta: 5min TTL para untouched pools
   - Falta: Adaptive TTL basado en pool weight (mayor weight = TTL más corto)

3. **Fuzzy Block Matching** ❌
   - Estado actual: No implementado
   - Falta: 5-block tolerance para cache hits
   - Falta: Batch optimization (priorizar touched pools, batch untouched pools)

4. **Hot Pool Manager Integration** ⚠️
   - Estado actual: Hot Pool Manager existe pero no se actualiza (0 pools)
   - Falta: Poblar Hot Pool Manager con top pools
   - Falta: Usar Hot Pool Manager para mejorar cache hit rate

---

### **2. JIT State Fetch Latency: ~547ms → ≤100ms** ❌

**Gap**: +447ms (4.5x más lento que objetivo)

**Causas Identificadas**:
1. ❌ **No hay local node integration**: Solo usa RPC remoto (Alchemy)
2. ❌ **Multicall batching subóptimo**: Puede no estar batching eficientemente
3. ❌ **Cache no optimizado**: Cache hit rate bajo (50%) significa más RPC calls

**Lo que falta implementar** (según propuesta):

1. **Local Node Integration** ❌
   - Estado actual: Solo RPC remoto
   - Falta: Auto-detection de local Reth/Geth nodes
   - Falta: Priority routing (local node → primary RPC → failover RPCs)
   - Falta: Connection pooling para local node con keep-alive connections
   - Impacto esperado: <10ms con local node (vs ~547ms actual)

2. **Multicall Batching Optimization** ⚠️
   - Estado actual: Multicall existe pero puede no estar optimizado
   - Falta: Batch size optimization (hasta 200 calls por batch según propuesta)
   - Falta: Priorización de touched pools sobre untouched pools

3. **Cache Optimization** ⚠️
   - Estado actual: Cache hit rate 50%
   - Falta: Mejorar cache hit rate a ≥80% (ver sección 1)
   - Impacto: Menos RPC calls = menor latencia promedio

---

### **3. End-to-End Latency: ~6,300ms → ≤200ms** ❌

**Gap**: +6,100ms (31.5x más lento que objetivo)

**Desglose Actual**:
- Discovery cycle: ~2,317ms
- Graph update: ~4,029ms
- **Total**: ~6,346ms

**Objetivo**: ≤200ms total

**Causas Identificadas**:
1. ❌ **Graph update muy lento**: 4.0s para 78 pools (debería ser <200ms total)
2. ❌ **Discovery cycle lento**: 2.3s por ciclo (puede optimizarse)
3. ❌ **RPC calls secuenciales**: Puede haber paralelización insuficiente
4. ❌ **Price fetching lento**: Histórico indica que price fetching es 60% del tiempo de graph update

**Lo que falta implementar** (según propuesta):

1. **Parallel Price Fetching** ❌
   - Estado actual: Price fetching probablemente secuencial
   - Falta: Parallel price fetching (reducción esperada: 40% según propuesta)
   - Impacto: Graph update de 4.0s → ~2.4s (si price fetching es 60% del tiempo)

2. **Batch Database Updates** ⚠️
   - Estado actual: Puede haber updates individuales
   - Falta: Batch database updates (reducción esperada: 50% overhead según propuesta)

3. **Cache Optimization** ⚠️
   - Estado actual: Cache hit rate 50%
   - Falta: Mejorar cache hit rate a ≥80%
   - Impacto: Menos RPC calls = menor latencia

4. **Local Node Integration** ❌
   - Estado actual: Solo RPC remoto
   - Falta: Local node integration
   - Impacto: RPC calls de ~547ms → <10ms (con local node)

---

### **4. RPC Calls per Block: ≤30** ✅

**Estado**: ✅ **CUMPLIDO**
- Actual: 20 calls en benchmark
- Objetivo: ≤30 calls
- **Gap**: 0 (ya cumplido)

**Nota**: Este objetivo ya se cumple, pero puede mejorarse aún más con cache optimization.

---

## 🚧 Componentes Faltantes (Según Propuesta)

### **Milestone 1 - Cache Optimization & State Synchronization**

#### **1. Merkle Tree-Based Cache Invalidation** ❌

**Estado**: No implementado

**Falta**:
- `src/cache/state_cache.rs`: Merkle-tree based state cache
- State hash calculation para V2 (reserves) y V3 (sqrt_price_x96, liquidity, tick)
- Cache invalidation solo cuando cambia el state hash (no block-based)
- Unit tests: Cache invalidation logic (property-based tests)

**Archivos a crear/modificar**:
- `src/cache/state_cache.rs` (nuevo)
- `src/jit_state_fetcher.rs` (modificar para usar state hash)
- `src/adapters/` (modificar para calcular state hash)

---

#### **2. TTL Differentiation Strategy** ❌

**Estado**: No implementado

**Falta**:
- TTL diferenciado: 30s para touched pools, 5min para untouched pools
- Adaptive TTL basado en pool weight
- Integración con event tracking (Swap/Mint/Burn events)

**Archivos a crear/modificar**:
- `src/hot_pool_manager.rs` (modificar para TTL diferenciado)
- `src/cache/` (nuevo módulo para TTL management)

---

#### **3. Fuzzy Block Matching** ❌

**Estado**: No implementado

**Falta**:
- 5-block tolerance para cache hits
- Batch optimization (priorizar touched pools)
- Lógica de fuzzy matching en JIT state fetcher

**Archivos a crear/modificar**:
- `src/jit_state_fetcher.rs` (modificar para fuzzy block matching)

---

#### **4. Local Node Integration** ❌

**Estado**: No implementado

**Falta**:
- Auto-detection de local Reth/Geth nodes
- Priority routing (local node → primary RPC → failover)
- Connection pooling para local node
- Configuration: `settings.toml` local node URL option

**Archivos a crear/modificar**:
- `src/rpc_pool.rs` (modificar para local node detection y prioritization)
- `src/settings.rs` (agregar configuración de local node)
- Integration tests: Local node fallback scenarios

---

#### **5. Multicall Batching Optimization** ⚠️

**Estado**: Parcialmente implementado

**Falta**:
- Optimizar batch size (hasta 200 calls por batch según propuesta)
- Priorización de touched pools sobre untouched pools
- Batch optimization en JIT state fetcher

**Archivos a crear/modificar**:
- `src/multicall.rs` (optimizar batch size)
- `src/jit_state_fetcher.rs` (integrar batch optimization)

---

#### **6. Parallel Price Fetching** ❌

**Estado**: No implementado

**Falta**:
- Parallel price fetching (reducción esperada: 40%)
- Integración con PriceFeed para parallel fetching

**Archivos a crear/modificar**:
- `src/price_feeds.rs` (modificar para parallel fetching)
- `src/graph_service.rs` (modificar para usar parallel price fetching)

---

#### **7. Batch Database Updates** ⚠️

**Estado**: Parcialmente implementado

**Falta**:
- Optimizar batch size para database updates
- Reducir overhead de database writes (reducción esperada: 50%)

**Archivos a crear/modificar**:
- `src/database.rs` (optimizar batch updates)
- `src/graph_service.rs` (usar batch updates)

---

## 📋 Checklist de Implementación

### **Prioridad P0 (Crítico para Milestone 1)**:

- [ ] **Merkle Tree-Based Cache Invalidation**
  - [ ] Crear `src/cache/state_cache.rs`
  - [ ] Implementar state hash calculation (V2 y V3)
  - [ ] Modificar cache invalidation para usar state hash
  - [ ] Unit tests (property-based tests)

- [ ] **TTL Differentiation Strategy**
  - [ ] Implementar TTL diferenciado (30s touched, 5min untouched)
  - [ ] Adaptive TTL basado en pool weight
  - [ ] Integración con event tracking

- [ ] **Fuzzy Block Matching**
  - [ ] Implementar 5-block tolerance
  - [ ] Batch optimization (priorizar touched pools)
  - [ ] Integrar en JIT state fetcher

- [ ] **Local Node Integration**
  - [ ] Auto-detection de local nodes
  - [ ] Priority routing
  - [ ] Connection pooling
  - [ ] Configuration y tests

### **Prioridad P1 (Importante para Performance)**:

- [ ] **Multicall Batching Optimization**
  - [ ] Optimizar batch size (hasta 200 calls)
  - [ ] Priorización de touched pools

- [ ] **Parallel Price Fetching**
  - [ ] Implementar parallel fetching
  - [ ] Integrar en graph service

- [ ] **Batch Database Updates**
  - [ ] Optimizar batch size
  - [ ] Reducir overhead

### **Prioridad P2 (Mejoras Adicionales)**:

- [ ] **Hot Pool Manager Population**
  - [ ] Poblar Hot Pool Manager con top pools
  - [ ] Integrar con cache para mejorar hit rate

- [ ] **Benchmarking y Validación**
  - [ ] Ejecutar benchmarks con optimizaciones
  - [ ] Validar que se cumplen objetivos (≥80% cache hit rate, ≤100ms latency, ≤200ms end-to-end)

---

## 🎯 Plan de Acción Recomendado

### **Fase 1: Cache Optimization (2-3 semanas)**
1. Implementar Merkle Tree-Based Cache Invalidation
2. Implementar TTL Differentiation Strategy
3. Implementar Fuzzy Block Matching
4. Validar cache hit rate ≥80%

### **Fase 2: Local Node Integration (1-2 semanas)**
1. Implementar auto-detection de local nodes
2. Implementar priority routing
3. Implementar connection pooling
4. Validar JIT latency ≤10ms (local node)

### **Fase 3: Performance Optimization (1-2 semanas)**
1. Optimizar multicall batching
2. Implementar parallel price fetching
3. Optimizar batch database updates
4. Validar end-to-end latency ≤200ms

### **Fase 4: Integration & Testing (1 semana)**
1. Integrar todas las optimizaciones
2. Ejecutar benchmarks completos (10,000 blocks)
3. Validar todos los objetivos de Milestone 1
4. Documentar resultados

---

## 📊 Métricas Esperadas Post-Implementación

### **Con Todas las Optimizaciones**:

| Métrica | Actual | Objetivo | Esperado Post-Optimización |
|---------|--------|----------|----------------------------|
| **Cache hit rate** | 50% | ≥80% | **≥80%** |
| **JIT latency (local)** | N/A | ≤10ms | **<10ms** |
| **JIT latency (remote)** | ~547ms | ≤100ms | **<100ms** |
| **RPC calls/block** | 20 | ≤30 | **<20** (mejorado) |
| **End-to-end latency** | ~6,300ms | ≤200ms | **<200ms** |

---

## ✅ Conclusión

**Estado General**: El servicio JIT actual está **parcialmente implementado** pero **falta optimización crítica** para alcanzar los objetivos de Milestone 1.

**Componentes Críticos Faltantes**:
1. ❌ Merkle Tree-Based Cache Invalidation
2. ❌ TTL Differentiation Strategy
3. ❌ Fuzzy Block Matching
4. ❌ Local Node Integration
5. ⚠️ Parallel Price Fetching
6. ⚠️ Batch Database Updates Optimization

**Tiempo Estimado**: 5-8 semanas para implementar todas las optimizaciones y alcanzar los objetivos de Milestone 1.

**Próximo Paso**: Comenzar con Fase 1 (Cache Optimization) que es la base para mejorar cache hit rate y reducir RPC calls.
