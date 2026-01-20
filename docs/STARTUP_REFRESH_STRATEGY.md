# Estrategia de Refresh al Inicio del Background Discoverer

**Fecha**: 18 de Enero, 2026  
**Análisis**: Estrategia óptima para refresh inicial vs rutina periódica

---

## 📊 Estado Actual vs Recomendado

### **Estado Actual** ⚠️
1. ✅ Corrige flags `is_active` basándose en weights existentes
2. ✅ Pobla Hot Pool Manager con `populate_hot_pool_manager_from_db()` que tiene:
   - `enable_fallback_refresh: true` → Ejecuta full refresh SOLO si no hay candidatos
   - Carga top 200 candidatos con weight >= $10K
   - Selecciona top 50 pools para Hot Pool Manager

**Problema**: Si hay candidatos pero con weights stale (> 24 horas), no los refresca.

---

### **Estrategia Recomendada** ✅ **IMPLEMENTADA**

```
┌─────────────────────────────────────────────────────────────┐
│              STARTUP SEQUENCE (Optimizado)                  │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  1. ⚡ Hot Refresh Inmediato (10-20s)                        │
│     → Actualiza top 50 pools críticos (weight >= $100K)     │
│     → Costo: ~$0.001                                         │
│     → Asegura pools más importantes frescos                 │
│                                                               │
│  2. 📦 Populate Hot Pool Manager (5-10s)                      │
│     → Carga desde BD con weights frescos (del paso 1)      │
│     → Fallback a full refresh SOLO si no hay candidatos     │
│                                                               │
│  3. 🚀 Spawn Tasks (inmediato)                               │
│     → Discovery cycles                                        │
│     → Graph updates incrementales                            │
│     → Hot refresh cada 30 min                                 │
│     → Warm refresh cada 1 hora                                │
│     → Full refresh cada 24 horas                             │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

---

## 🎯 Análisis de Opciones

### **Opción 1: Full Refresh al Inicio** ❌ NO RECOMENDADO

**Implementación**:
```rust
graph_service.calculate_and_update_all_weights().await?;
populate_hot_pool_manager_from_db(...).await?;
```

**Pros**:
- ✅ Asegura que todos los weights están frescos desde el inicio

**Contras**:
- ❌ **Muy lento**: ~40K RPC calls = ~5-10 minutos de startup
- ❌ **Bloquea startup**: No puede servir requests hasta completar
- ❌ **Puede ser innecesario**: Si ya hay weights válidos (< 24 horas)
- ❌ **Alto costo RPC**: ~$0.40 por startup
- ❌ **Si falla**: Todo el servicio falla al iniciar

**Costo**: ~$0.40 por startup + 5-10 minutos de latencia

---

### **Opción 2: Hot Refresh al Inicio** ✅ RECOMENDADO (IMPLEMENTADA)

**Implementación**:
```rust
weight_refresher::refresh_hot_pools(
    &graph_service,
    &db_pool,
    rpc_pool.clone(),
    50,        // top 50 pools
    100_000.0, // min weight: $100K
    Some(flight_recorder.clone()),
).await?;

populate_hot_pool_manager_from_db(...).await?;
```

**Pros**:
- ✅ **Rápido**: ~100 RPC calls = ~10-20 segundos
- ✅ **No bloquea**: Startup rápido, servicio disponible inmediatamente
- ✅ **Asegura pools críticos frescos**: Los pools más importantes están actualizados
- ✅ **Bajo costo**: ~$0.001 por startup
- ✅ **Resiliente**: Si falla, puede continuar con weights existentes

**Contras**:
- ⚠️ No actualiza todos los pools (solo top 50)
- ⚠️ Pools menos importantes pueden tener weights stale

**Costo**: ~$0.001 por startup + 10-20 segundos de latencia

---

### **Opción 3: Solo Populate con Fallback (Actual)** ⚠️ SUBÓPTIMO

**Estado Actual**:
```rust
populate_hot_pool_manager_from_db(
    ...,
    enable_fallback_refresh: true,
).await?;
```

**Pros**:
- ✅ Simple
- ✅ Fallback a full refresh si no hay candidatos

**Contras**:
- ❌ **No actualiza weights stale**: Si hay candidatos pero con weights antiguos (> 24 horas), no los refresca
- ❌ **Hot Pool Manager puede tener weights obsoletos**: Pools críticos pueden tener weights de hace días
- ❌ **Primera request puede ser lenta**: Si weights están stale, primera operación puede fallar

---

## 📊 Comparación de Estrategias

| Estrategia | Startup Time | Costo RPC | Pools Frescos | Resiliencia | Estado |
|------------|-------------|-----------|---------------|-------------|--------|
| **Full Refresh** | 5-10 min | ~$0.40 | Todos | Baja | ❌ No recomendado |
| **Hot Refresh** | 10-20s | ~$0.001 | Top 50 | Alta | ✅ **IMPLEMENTADO** |
| **Solo Populate** | 5-10s | ~$0 | Ninguno* | Media | ⚠️ Subóptimo |

*Ninguno si hay candidatos stale en BD

---

## ✅ Implementación

### **Cambios en `background_discoverer.rs`**:

**Después de corregir flags `is_active`** (línea ~281):

```rust
// ✅ ESTRATEGIA HÍBRIDA: Hot refresh inmediato al inicio (pools críticos frescos)
// Esto asegura que los top 50 pools más importantes tengan weights frescos
// sin bloquear el startup por 5-10 minutos como haría un full refresh
println!("🔥 Starting initial hot pools refresh (top 50 pools, weight >= $100K)...");
match weight_refresher::refresh_hot_pools(
    &graph_service,
    &db_pool,
    rpc_pool.clone(),
    50,        // top 50 pools
    100_000.0, // min weight: $100K
    Some(flight_recorder_arc.clone()),
).await {
    Ok(count) => {
        println!("✅ Initial hot pools refresh completed: {} pools updated", count);
    }
    Err(e) => {
        eprintln!("⚠️ Initial hot pools refresh failed: {} (continuing with existing weights)", e);
    }
}

// ✅ MEJORADO: Poblar Hot Pool Manager (ahora con weights frescos del hot refresh)
// El fallback a full refresh solo se ejecuta si realmente no hay candidatos
println!("🔄 Populating Hot Pool Manager from database...");
match populate_hot_pool_manager_from_db(
    &hot_pool_manager,
    &*graph_service,
    &db_pool,
    rpc_pool.clone(),
).await {
    Ok(count) => {
        println!("✅ Hot Pool Manager populated with {} pools", count);
    }
    Err(e) => {
        eprintln!("❌ Failed to populate Hot Pool Manager: {} (continuing anyway)", e);
    }
}
```

---

## 🎯 Ventajas de la Estrategia Implementada

### **1. Startup Rápido** ✅
- **Tiempo total**: ~15-30 segundos (vs 5-10 minutos con full refresh)
- **Servicio disponible**: Inmediatamente después del startup
- **No bloquea**: Puede servir requests mientras ejecuta tasks en background

### **2. Pools Críticos Frescos** ✅
- **Top 50 pools**: Actualizados inmediatamente al inicio
- **Weight >= $100K**: Solo pools más importantes
- **Costo bajo**: ~$0.001 vs ~$0.40 con full refresh

### **3. Resiliencia** ✅
- **Si falla hot refresh**: Continúa con weights existentes
- **Fallback inteligente**: Full refresh solo si realmente no hay candidatos
- **No bloquea startup**: Servicio siempre inicia, incluso si refresh falla

### **4. Rutina Periódica Mantenida** ✅
- **Hot refresh**: Cada 30 minutos (mantiene pools críticos frescos)
- **Warm refresh**: Cada 1 hora (pools medianos)
- **Full refresh**: Cada 24 horas (sincronización completa)

---

## 📊 Flujo Completo

```
STARTUP (15-30 segundos)
├─ 1. Corregir flags is_active (5s)
├─ 2. Hot refresh inmediato (10-20s) ← NUEVO
│   └─ Actualiza top 50 pools críticos
├─ 3. Populate Hot Pool Manager (5-10s)
│   └─ Con fallback a full refresh solo si no hay candidatos
└─ 4. Spawn tasks (inmediato)
    ├─ Discovery cycles
    ├─ Graph updates incrementales
    ├─ Hot refresh cada 30 min
    ├─ Warm refresh cada 1 hora
    └─ Full refresh cada 24 horas
```

---

## ✅ Conclusión

### **Recomendación Final**: **Hot Refresh al Inicio** ✅ IMPLEMENTADO

**Razones**:
1. ✅ Startup rápido (~15-30 segundos total)
2. ✅ Pools críticos frescos desde el inicio
3. ✅ Bajo costo (~$0.001 normalmente)
4. ✅ Resiliente (puede continuar si falla)
5. ✅ Mejor UX (servicio disponible rápidamente)

**NO hacer full refresh al inicio** porque:
- ❌ Muy lento (5-10 minutos)
- ❌ Bloquea startup
- ❌ Puede ser innecesario si ya hay weights válidos
- ❌ Alto costo RPC (~$0.40)

**Orden de Ejecución Implementado**:
1. ⚡ Hot refresh inmediato (top 50 pools) ← **NUEVO**
2. 📦 Populate Hot Pool Manager (con fallback)
3. 🚀 Spawn tasks con rutina normal (30 min, 1 hora, 24 horas)

---

## 📝 Próximos Pasos

1. ✅ **Implementado**: Hot refresh inmediato al inicio
2. ✅ **Mantenido**: Populate con fallback
3. ✅ **Mantenido**: Rutina periódica normal
4. ✅ **Agregado**: Logging detallado del proceso de startup

**Estado**: ✅ **IMPLEMENTADO Y LISTO PARA PROBAR**
