# Instructivo: Refactorización del Poblamiento del Hot Pool Manager

## Objetivo
Separar el poblamiento del Hot Pool Manager del cálculo de pesos, siguiendo el patrón del bot original. Esto permite poblar el Hot Pool Manager desde la base de datos con pesos ya calculados, independientemente de si el cálculo de pesos funciona correctamente en tiempo real.

---

## PASO 1: Remover código de poblamiento de `calculate_and_update_all_weights`

**Archivo:** `MIG_SDK_EXPORT/src/graph_service.rs`

**Ubicación:** Líneas 616-646 (aproximadamente)

**Acción:** Eliminar el bloque completo que intenta poblar el Hot Pool Manager durante el cálculo de pesos.

**Código a ELIMINAR:**
```rust
        // ✅ HOT POOL MANAGER: Add top pools to Hot Pool Manager based on weights
        if let Some(ref hot_pool_manager) = self.hot_pool_manager {
            // Collect pools with their weights and sort by weight (descending)
            let mut pools_with_weights: Vec<(Address, f64, Pool)> = Vec::new();
            for pool in &pools_for_hot_manager {
                let addr = pool.address();
                if let Some(weight) = self.weights.get(&addr) {
                    if *weight > 0.0 {
                        pools_with_weights.push((addr, *weight, pool.clone()));
                    }
                }
            }
            
            // Sort by weight descending and take top pools
            pools_with_weights.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            
            // Add top pools to Hot Pool Manager (limit to top 200 to avoid memory issues)
            let top_pools_to_add = pools_with_weights.iter().take(200);
            let mut added_count = 0;
            for (addr, weight, pool) in top_pools_to_add {
                // Only add if not already in hot pools
                if !hot_pool_manager.is_hot_pool(*addr) {
                    hot_pool_manager.add_hot_pool(&pool, *weight);
                    added_count += 1;
                }
            }
            
            if added_count > 0 {
                info!("✅ Added {} top pools to Hot Pool Manager (sorted by weight)", added_count);
            }
        }
```

**También eliminar la variable `pools_for_hot_manager` que ya no se necesita:**

**Ubicación:** Línea 505-506 (aproximadamente)

**Código a ELIMINAR:**
```rust
        // ✅ Save pools for Hot Pool Manager (before consuming in loop)
        let pools_for_hot_manager = pools_with_state_vec.clone();
```

**Nota:** La variable `pools_with_state_vec` se sigue usando en el loop de cálculo de pesos, así que NO eliminar esa.

---

## PASO 2: Agregar función `load_pool_candidates` en `database.rs`

**Archivo:** `MIG_SDK_EXPORT/src/database.rs`

**Ubicación:** Después de la función `load_all_graph_weights` (aproximadamente línea 824)

**Acción:** Agregar la siguiente función y estructura auxiliar.

**Código a AGREGAR:**
```rust
/// Pool candidate loaded from database with weight information
#[derive(Debug, Clone)]
pub struct PoolCandidate {
    pub address: Address,
    pub dex: String,
    pub token0: Address,
    pub token1: Address,
    pub fee_bps: u32,
    pub weight: f64,
    pub last_update_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Load pool candidates from database using optimized query with graph_weights
/// 
/// Returns pools with weight >= $100K, ordered by freshness and weight.
/// This is used to populate the Hot Pool Manager with high-quality pools.
pub async fn load_pool_candidates(
    pool: &DbPool,
) -> Result<Vec<PoolCandidate>> {
    use chrono::Utc;
    
    // Criterios de selección:
    // 1. Mínimo $100K USD en liquidez (solo pools de alta calidad)
    // 2. Preferir weights frescos, pero aceptar hasta 7 días si no hay frescos
    // 3. Limitar a top 200 candidatos (luego filtraremos a top 50 hot)
    let min_weight = 100_000.0;  // Mínimo $100K USD en liquidez
    let max_age_fresh = Utc::now() - chrono::Duration::minutes(5);  // Preferir weights frescos
    let max_age_acceptable = Utc::now() - chrono::Duration::days(7);  // Aceptar hasta 7 días si no hay frescos
    let limit = 200;  // Limitar candidatos iniciales
    
    // Query optimizada con INNER JOIN para usar índice en graph_weights.weight
    let rows = sqlx::query(
        &format!(
            r#"
            SELECT 
                p.address,
                p.dex,
                p.token0,
                p.token1,
                p.fee_bps,
                gw.weight,
                gw.updated_at as last_update_timestamp
            FROM {}.pools p
            INNER JOIN {}.graph_weights gw ON p.address = gw.pool_address
            WHERE p.is_active = true
              AND gw.weight >= $1
              AND (gw.updated_at IS NULL OR gw.updated_at >= $2 OR gw.updated_at >= $3)
            ORDER BY 
              -- Priorizar weights frescos primero, luego por liquidez
              CASE 
                WHEN gw.updated_at IS NULL THEN 0
                WHEN gw.updated_at >= $2 THEN 1
                ELSE 2
              END,
              gw.weight DESC,  -- Prioridad 2: Liquidez (mejores primero)
              p.last_seen_block DESC          -- Prioridad 3: Frescura del pool
            LIMIT $4
            "#,
            SCHEMA, SCHEMA
        ),
    )
    .bind(min_weight)
    .bind(max_age_fresh)
    .bind(max_age_acceptable)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    
    let mut candidates = Vec::new();
    for row in rows {
        let address: String = row.try_get("address")?;
        let address = address.parse::<Address>()?;
        let dex: String = row.try_get("dex")?;
        let token0: String = row.try_get("token0")?;
        let token0 = token0.parse::<Address>()?;
        let token1: String = row.try_get("token1")?;
        let token1 = token1.parse::<Address>()?;
        let fee_bps = row.try_get::<Option<i32>, _>("fee_bps")?.unwrap_or(30) as u32;
        let weight: f64 = row.try_get("weight")?;
        let last_update_timestamp: Option<chrono::DateTime<chrono::Utc>> = row.try_get("last_update_timestamp")?;
        
        candidates.push(PoolCandidate {
            address,
            dex,
            token0,
            token1,
            fee_bps,
            weight,
            last_update_timestamp,
        });
    }
    
    Ok(candidates)
}
```

**Importante:** Asegúrate de que el módulo `database.rs` tenga acceso a `chrono`. Si no está importado, agregar al inicio del archivo:
```rust
use chrono::{DateTime, Utc};
```

---

## PASO 3: Agregar estructuras y funciones auxiliares en `background_discoverer.rs`

**Archivo:** `MIG_SDK_EXPORT/bin/background_discoverer.rs`

**Ubicación:** Después de los imports (aproximadamente después de la línea 50), antes de la función `main`

**Acción:** Agregar las siguientes estructuras y funciones auxiliares.

**Código a AGREGAR:**
```rust
use std::time::Instant;
use ethers::types::Bytes;
use ethers::prelude::abigen;
use tokio::time::{timeout, Duration};

/// Validated pool candidate with initial state
#[derive(Debug, Clone)]
struct ValidatedPoolCandidate {
    candidate: database::PoolCandidate,
    initial_state: mig_topology_sdk::hot_pool_manager::PoolInitialState,
}

/// Create Pool enum from PoolCandidate metadata
fn create_pool_from_metadata(candidate: &database::PoolCandidate) -> Result<mig_topology_sdk::pools::Pool> {
    use mig_topology_sdk::pools::{Pool, UniswapV2Pool, UniswapV3Pool};
    use ethers::types::U256;
    
    let address = candidate.address;
    let token0 = candidate.token0;
    let token1 = candidate.token1;
    let fee_bps = candidate.fee_bps;
    
    let pool = if candidate.dex.contains("Curve") {
        use mig_topology_sdk::pools::CurveStableSwapPool;
        Pool::CurveStableSwap(CurveStableSwapPool {
            address,
            tokens: vec![token0, token1],
            balances: vec![U256::zero(), U256::zero()],
            a: U256::zero(),
            fee: U256::from(fee_bps),
            dex: "Curve",
        })
    } else if candidate.dex.contains("Balancer") {
        use mig_topology_sdk::pools::BalancerWeightedPool;
        let mut pool_id = [0u8; 32];
        pool_id[0..20].copy_from_slice(&address.as_bytes());
        Pool::BalancerWeighted(BalancerWeightedPool {
            address,
            pool_id,
            tokens: vec![token0, token1],
            weights: vec![U256::from(500000u64), U256::from(500000u64)],
            balances: vec![U256::zero(), U256::zero()],
            swap_fee: U256::from(fee_bps),
            dex: "Balancer",
        })
    } else if candidate.dex.contains("V3") || candidate.dex == "KyberSwap" {
        Pool::UniswapV3(UniswapV3Pool {
            address,
            token0,
            token1,
            fee: fee_bps,
            sqrt_price_x96: U256::zero(),
            liquidity: 0,
            tick: 0,
            dex: if candidate.dex == "UniswapV3" { "UniswapV3" } 
                else if candidate.dex == "CamelotV3" { "CamelotV3" }
                else { "UniswapV3" },
        })
    } else {
        Pool::UniswapV2(UniswapV2Pool {
            address,
            token0,
            token1,
            reserve0: 0,
            reserve1: 0,
            dex: if candidate.dex == "SushiSwapV2" { "SushiSwapV2" }
                else if candidate.dex == "CamelotV2" { "CamelotV2" }
                else { "UniswapV2" },
        })
    };
    
    Ok(pool)
}

/// Validate pools batch by checking on-chain state
/// 
/// Valida pools verificando que respondan correctamente a queries on-chain.
/// Para V3: verifica slot0() y liquidity() != 0
/// Para V2: verifica getReserves() con ambas reservas > 0
async fn validate_pools_batch(
    candidates: &[database::PoolCandidate],
    rpc_pool: Arc<RpcPool>,
    settings: &Settings,
) -> Result<Vec<ValidatedPoolCandidate>> {
    use mig_topology_sdk::multicall::{Multicall, Call};
    use mig_topology_sdk::contracts::{IUniswapV2Pair, UniswapV3Pool};
    use std::collections::HashMap;
    
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    
    // Validar en batches de 50 pools por multicall
    let batches: Vec<_> = candidates.chunks(50).collect();
    let mut validated = Vec::new();
    
    // Multicall address from settings
    let multicall_address = Address::from_str(&settings.contracts.factories.multicall)?;
    let multicall_batch_size = settings.performance.multicall_batch_size;
    
    for batch in batches {
        let (provider, _permit) = rpc_pool.get_next_provider().await?;
        let provider_arc = Arc::new(provider);
        let multicall = Multicall::new(provider_arc.clone(), multicall_address, multicall_batch_size);
        
        let mut calls = Vec::new();
        let mut call_map = Vec::new(); // (pool_index, call_type: "v2" | "v3_slot0" | "v3_liquidity")
        
        // Preparar calls según tipo de pool
        for (pool_idx, pool) in batch.iter().enumerate() {
            match pool.dex.as_str() {
                dex if dex.contains("V3") => {
                    // V3: slot0() y liquidity()
                    let v3_pool = UniswapV3Pool::new(pool.address, provider_arc.clone());
                    match (v3_pool.slot_0().calldata(), v3_pool.liquidity().calldata()) {
                        (Some(slot0_calldata), Some(liquidity_calldata)) => {
                            calls.push(Call {
                                target: pool.address,
                                call_data: slot0_calldata,
                            });
                            call_map.push((pool_idx, "v3_slot0"));
                            calls.push(Call {
                                target: pool.address,
                                call_data: liquidity_calldata,
                            });
                            call_map.push((pool_idx, "v3_liquidity"));
                        }
                        _ => {
                            tracing::debug!("Failed to get calldata for V3 pool {}", pool.address);
                        }
                    }
                }
                _ => {
                    // V2: getReserves()
                    let v2_pair = IUniswapV2Pair::new(pool.address, provider_arc.clone());
                    if let Some(reserves_calldata) = v2_pair.get_reserves().calldata() {
                        calls.push(Call {
                            target: pool.address,
                            call_data: reserves_calldata,
                        });
                        call_map.push((pool_idx, "v2"));
                    }
                }
            }
        }
        
        if calls.is_empty() {
            // Si no se pudieron preparar calls, NO asumir válidos - skip batch
            continue;
        }
        
        // Ejecutar multicall con timeout
        let results = match timeout(
            Duration::from_secs(2),
            multicall.run(calls, None)
        ).await {
            Ok(Ok(results)) => results,
            Ok(Err(e)) => {
                tracing::warn!("⚠️ Multicall failed for batch: {:?}", e);
                continue;
            }
            Err(_) => {
                tracing::warn!("⚠️ Multicall timeout for batch");
                continue;
            }
        };
        
        // Crear dummy contracts para acceder a ABI functions
        let v2_dummy = IUniswapV2Pair::new(Address::zero(), provider_arc.clone());
        let v3_dummy = UniswapV3Pool::new(Address::zero(), provider_arc.clone());
        
        // Obtener ABI functions
        let get_reserves_fn = match v2_dummy.abi().function("getReserves") {
            Ok(f) => f,
            Err(_) => continue,
        };
        
        let slot0_fn = match v3_dummy.abi().function("slot0") {
            Ok(f) => f,
            Err(_) => continue,
        };
        
        let liquidity_fn = match v3_dummy.abi().function("liquidity") {
            Ok(f) => f,
            Err(_) => continue,
        };
        
        // Procesar resultados y validar pools
        let mut pool_results: HashMap<usize, (Option<Bytes>, Option<Bytes>)> = HashMap::new();
        
        for (call_idx, (pool_idx, call_type)) in call_map.iter().enumerate() {
            if let Some(result) = results.get(call_idx) {
                if result.is_empty() {
                    continue; // Skip empty results
                }
                let entry = pool_results.entry(*pool_idx).or_insert((None, None));
                match *call_type {
                    "v3_slot0" => entry.0 = Some(result.clone()),
                    "v3_liquidity" => entry.1 = Some(result.clone()),
                    "v2" => {
                        // Para V2, solo necesitamos un resultado
                        entry.0 = Some(result.clone());
                    }
                    _ => {}
                }
            }
        }
        
        // Función auxiliar para validar y extraer estado inicial de un pool V3
        let validate_and_extract_v3 = |slot0_data: &Bytes, liquidity_data: &Bytes, pool_addr: Address| -> Option<mig_topology_sdk::hot_pool_manager::PoolInitialState> {
            let slot0_decoded = match slot0_fn.decode_output(slot0_data.as_ref()) {
                Ok(decoded) => decoded,
                Err(_) => return None,
            };
            
            let liquidity_decoded = match liquidity_fn.decode_output(liquidity_data.as_ref()) {
                Ok(decoded) => decoded,
                Err(_) => return None,
            };
            
            // Extraer y validar: sqrt_price_x96 != 0 y liquidity > 0
            let sqrt_price_x96_opt = slot0_decoded[0].clone().into_uint();
            let tick_opt: Option<i32> = slot0_decoded[1].clone().into_int().and_then(|i| i.try_into().ok());
            let liquidity_opt = liquidity_decoded[0].clone().into_uint().and_then(|u| {
                let u128_val = u.as_u128();
                if u128_val > 0 { Some(u128_val) } else { None }
            });
            
            match (sqrt_price_x96_opt, liquidity_opt, tick_opt) {
                (Some(sqrt_price_x96), Some(liquidity), Some(tick)) => {
                    if !sqrt_price_x96.is_zero() && liquidity > 0 {
                        Some(mig_topology_sdk::hot_pool_manager::PoolInitialState::V3 {
                            sqrt_price_x96,
                            tick: tick as i64,
                            liquidity,
                        })
                    } else {
                        None
                    }
                }
                _ => None
            }
        };
        
        // Función auxiliar para validar y extraer estado inicial de un pool V2
        let validate_and_extract_v2 = |reserves_data: &Bytes, pool_addr: Address| -> Option<mig_topology_sdk::hot_pool_manager::PoolInitialState> {
            let reserves_decoded = match get_reserves_fn.decode_output(reserves_data.as_ref()) {
                Ok(decoded) => decoded,
                Err(_) => return None,
            };
            
            // Extraer y validar: reserve0 > 0 y reserve1 > 0
            let reserve0_opt = reserves_decoded[0].clone().into_uint();
            let reserve1_opt = reserves_decoded[1].clone().into_uint();
            
            match (reserve0_opt, reserve1_opt) {
                (Some(reserve0), Some(reserve1)) => {
                    if !reserve0.is_zero() && !reserve1.is_zero() {
                        Some(mig_topology_sdk::hot_pool_manager::PoolInitialState::V2 {
                            reserve0,
                            reserve1,
                        })
                    } else {
                        None
                    }
                }
                _ => None
            }
        };
        
        // Validar cada pool según sus resultados parseados y extraer estado inicial
        for (i, pool) in batch.iter().enumerate() {
            let initial_state_opt = match pool.dex.as_str() {
                dex if dex.contains("V3") => {
                    // V3: parsear y validar slot0 y liquidity
                    if let Some((Some(slot0_data), Some(liquidity_data))) = pool_results.get(&i) {
                        validate_and_extract_v3(slot0_data, liquidity_data, pool.address)
                    } else {
                        None
                    }
                }
                _ => {
                    // V2: parsear y validar reserves
                    if let Some((Some(reserves_data), _)) = pool_results.get(&i) {
                        validate_and_extract_v2(reserves_data, pool.address)
                    } else {
                        None
                    }
                }
            };
            
            if let Some(initial_state) = initial_state_opt {
                validated.push(ValidatedPoolCandidate {
                    candidate: pool.clone(),
                    initial_state,
                });
            } else {
                tracing::debug!("Pool {} failed validation (invalid state)", pool.address);
            }
        }
    }
    
    Ok(validated)
}
```

**Importante:** Asegúrate de que los imports necesarios estén al inicio del archivo. Si faltan, agregar:
```rust
use std::collections::HashMap;
use ethers::types::Bytes;
use tokio::time::{timeout, Duration};
```

---

## PASO 4: Crear función `populate_hot_pool_manager_from_db` en `background_discoverer.rs`

**Archivo:** `MIG_SDK_EXPORT/bin/background_discoverer.rs`

**Ubicación:** Después de la función `validate_pools_batch` (antes de la función `main`)

**Acción:** Agregar la función principal que pobla el Hot Pool Manager desde la base de datos.

**Código a AGREGAR:**
```rust
/// Load pools from PostgreSQL database and populate HotPoolManager
/// 
/// ESTRATEGIA OPTIMIZADA:
/// 1. Carga candidatos desde BD usando query optimizada con graph_weights
/// 2. Split en Hot (weight >= $100K) y Warm (resto)
/// 3. Valida HOT pools SÍNCRONAMENTE (antes de construir rutas)
/// 4. Valida WARM pools en BACKGROUND (no bloquea)
/// 5. Carga solo pools válidos en HotPoolManager
async fn populate_hot_pool_manager_from_db(
    hot_pool_manager: &HotPoolManager,
    db_pool: &database::DbPool,
    rpc_pool: Arc<RpcPool>,
    settings: &Settings,
) -> Result<usize> {
    use std::sync::atomic::{AtomicU64, Ordering};
    
    // 🔍 GUARD: Verificar count al inicio
    let v3_count_at_entry = hot_pool_manager.v3_hot_pools.len();
    let v2_count_at_entry = hot_pool_manager.v2_hot_pools.len();
    tracing::warn!("🔍 populate_hot_pool_manager_from_db ENTRY: {} V3 pools, {} V2 pools", 
          v3_count_at_entry, v2_count_at_entry);
    
    let start = Instant::now();
    
    // 1. Cargar candidatos desde BD (query optimizada)
    let candidates = database::load_pool_candidates(db_pool).await?;
    tracing::info!("📦 Loaded {} pool candidates from database in {:?}", 
          candidates.len(), start.elapsed());
    
    if candidates.is_empty() {
        tracing::warn!("⚠️ No pool candidates found. Check database and graph_weights table.");
        return Ok(0);
    }
    
    // 2. Filtrar y ordenar: solo los mejores pools
    // Criterios: weight >= $100K, ordenados por weight DESC
    let mut hot_candidates: Vec<_> = candidates
        .into_iter()
        .filter(|p| p.weight >= 100_000.0)  // Solo pools con weight >= $100K
        .collect();
    
    // Ordenar por weight DESC (mejores primero)
    hot_candidates.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    
    let total_candidates = hot_candidates.len();
    
    // 3. Validar pools en batches hasta tener 50 pools válidos (o todos los disponibles si hay menos)
    // ESTRATEGIA: Seleccionar más candidatos iniciales (hasta 100) y validar hasta tener 50 válidos
    let target_valid_pools = 50;
    let max_candidates_to_validate = 100; // Validar hasta 100 candidatos para asegurar 50 válidos
    
    let candidates_to_validate: Vec<_> = hot_candidates
        .iter()
        .take(max_candidates_to_validate)
        .cloned()
        .collect();
    
    let remaining_candidates = if hot_candidates.len() > max_candidates_to_validate {
        hot_candidates.split_off(max_candidates_to_validate)
    } else {
        Vec::new()
    };
    
    tracing::info!("📊 Validating top {} candidates (out of {} with weight >= $100K) to get {} valid pools", 
          candidates_to_validate.len(), total_candidates, target_valid_pools);
    
    // Validar HOT pools SÍNCRONAMENTE (antes de construir rutas)
    let validate_start = Instant::now();
    let mut validated_hot = validate_pools_batch(
        &candidates_to_validate,
        rpc_pool.clone(),
        settings,
    ).await?;
    
    // Si no tenemos suficientes pools válidos, validar más candidatos
    let mut warm_candidates = remaining_candidates;
    let mut validated_count = validated_hot.len();
    let mut batch_num = 1;
    
    while validated_count < target_valid_pools && !warm_candidates.is_empty() && batch_num < 5 {
        // Tomar el siguiente batch de candidatos
        let next_batch_size = (target_valid_pools - validated_count).min(50).min(warm_candidates.len());
        let next_batch: Vec<_> = warm_candidates
            .drain(..next_batch_size)
            .collect();
        
        if next_batch.is_empty() {
            break;
        }
        
        tracing::info!("📊 Validating additional batch #{}: {} candidates to reach target of {} valid pools", 
              batch_num + 1, next_batch.len(), target_valid_pools);
        
        match validate_pools_batch(&next_batch, rpc_pool.clone(), settings).await {
            Ok(mut additional_validated) => {
                validated_hot.append(&mut additional_validated);
                validated_count = validated_hot.len();
                tracing::info!("✅ After batch #{}: {} valid pools (target: {})", 
                      batch_num + 1, validated_count, target_valid_pools);
            }
            Err(e) => {
                tracing::warn!("⚠️ Failed to validate additional batch #{}: {}", batch_num + 1, e);
                break;
            }
        }
        
        batch_num += 1;
    }
    
    if validated_hot.len() < target_valid_pools {
        tracing::info!("✅ Validated {} hot pools in {:?} (target was {}, but only {} candidates available)", 
              validated_hot.len(), 
              validate_start.elapsed(),
              target_valid_pools,
              total_candidates);
        tracing::info!("💡 Note: Background discoverer needs to map more pools to reach target of {} valid pools", target_valid_pools);
    } else {
        tracing::info!("✅ Validated {} hot pools in {:?} (target was {})", 
              validated_hot.len(), 
              validate_start.elapsed(),
              target_valid_pools);
    }
    
    // 4. Cargar hot pools validados en HotPoolManager CON ESTADO INICIAL VÁLIDO
    let mut pools_added = 0;
    for validated in &validated_hot {
        match create_pool_from_metadata(&validated.candidate) {
            Ok(pool) => {
                // Usar weight del candidato (no 1.0) para mejor priorización
                let pool_weight = validated.candidate.weight.max(100_000.0); // Mínimo $100K
                
                // ✅ FIX: Usar add_hot_pool_with_state con estado inicial válido
                if let Err(e) = hot_pool_manager.add_hot_pool_with_state(&pool, pool_weight, validated.initial_state.clone()) {
                    tracing::warn!("⚠️ Failed to add pool {} to HotPoolManager: {}", validated.candidate.address, e);
                } else {
                    pools_added += 1;
                }
            }
            Err(e) => {
                tracing::warn!("⚠️ Failed to create pool from metadata for {}: {}", validated.candidate.address, e);
            }
        }
    }
    
    tracing::info!("✅ Added {} hot pools to HotPoolManager", pools_added);
    
    // 🔍 GUARD: Verificar count al final
    let v3_count_at_exit = hot_pool_manager.v3_hot_pools.len();
    let v2_count_at_exit = hot_pool_manager.v2_hot_pools.len();
    tracing::warn!("🔍 populate_hot_pool_manager_from_db EXIT: {} V3 pools, {} V2 pools (delta: V3={}, V2={}, pools_added={})", 
          v3_count_at_exit, v2_count_at_exit,
          v3_count_at_exit as i64 - v3_count_at_entry as i64,
          v2_count_at_exit as i64 - v2_count_at_entry as i64,
          pools_added);
    
    if v3_count_at_exit < v3_count_at_entry || v2_count_at_exit < v2_count_at_entry {
        let v3_lost = v3_count_at_entry.saturating_sub(v3_count_at_exit);
        let v2_lost = v2_count_at_entry.saturating_sub(v2_count_at_exit);
        tracing::error!("🚨 CRITICAL: POOLS LOST INSIDE populate_hot_pool_manager_from_db! Lost {} V3 pools and {} V2 pools!", 
              v3_lost, v2_lost);
    }
    
    tracing::info!("📊 Total time to load and validate pools: {:?}", start.elapsed());
    Ok(pools_added)
}
```

---

## PASO 5: Integrar `populate_hot_pool_manager_from_db` en el ciclo de actualización de pesos

**Archivo:** `MIG_SDK_EXPORT/bin/background_discoverer.rs`

**Ubicación:** Dentro de la función `main`, en el task de actualización de pesos (aproximadamente línea 296-306)

**Acción:** Modificar el task para que llame a `populate_hot_pool_manager_from_db` después de actualizar los pesos.

**Código ACTUAL (aproximadamente líneas 296-306):**
```rust
    // 17. Spawn graph update task
    let graph_service_clone = Arc::clone(&graph_service);
    let graph_update_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(graph_update_interval));
        loop {
            interval.tick().await;
            println!("🔄 Updating graph weights...");
            match graph_service_clone.calculate_and_update_all_weights().await {
                Ok(_) => println!("✅ Graph weights updated"),
                Err(e) => eprintln!("❌ Graph weights update failed: {}", e),
            }
        }
    });
```

**Código NUEVO (reemplazar el task completo):**
```rust
    // 17. Spawn graph update task
    let graph_service_clone = Arc::clone(&graph_service);
    let hot_pool_manager_clone = Arc::clone(&hot_pool_manager);
    let db_pool_clone = db_pool.clone();
    let rpc_pool_clone = Arc::clone(&rpc_pool);
    let settings_clone = Arc::new(settings.clone());
    let graph_update_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(graph_update_interval));
        loop {
            interval.tick().await;
            println!("🔄 Updating graph weights...");
            match graph_service_clone.calculate_and_update_all_weights().await {
                Ok(_) => {
                    println!("✅ Graph weights updated");
                    
                    // Poblar Hot Pool Manager desde BD después de actualizar pesos
                    println!("🔄 Populating Hot Pool Manager from database...");
                    match populate_hot_pool_manager_from_db(
                        &hot_pool_manager_clone,
                        &db_pool_clone,
                        rpc_pool_clone.clone(),
                        &settings_clone,
                    ).await {
                        Ok(count) => println!("✅ Hot Pool Manager populated with {} pools", count),
                        Err(e) => eprintln!("❌ Failed to populate Hot Pool Manager: {}", e),
                    }
                }
                Err(e) => eprintln!("❌ Graph weights update failed: {}", e),
            }
        }
    });
```

**Importante:** Asegúrate de que `hot_pool_manager` esté disponible en el scope donde se crea este task. Si no está, necesitarás mover la creación del Hot Pool Manager antes de este punto.

---

## PASO 6: Verificar que `PoolCandidate` esté exportado en `database.rs`

**Archivo:** `MIG_SDK_EXPORT/src/database.rs`

**Ubicación:** Verificar que la estructura `PoolCandidate` sea pública (tiene `pub`)

**Acción:** Si la estructura no tiene `pub`, agregarlo:
```rust
/// Pool candidate loaded from database with weight information
#[derive(Debug, Clone)]
pub struct PoolCandidate {
    pub address: Address,
    pub dex: String,
    pub token0: Address,
    pub token1: Address,
    pub fee_bps: u32,
    pub weight: f64,
    pub last_update_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}
```

---

## PASO 7: Verificar imports en `lib.rs`

**Archivo:** `MIG_SDK_EXPORT/src/lib.rs`

**Acción:** Verificar que `database` esté exportado correctamente y que `PoolCandidate` sea accesible.

**Verificar que exista:**
```rust
pub mod database;
```

Y que `PoolCandidate` esté re-exportado si es necesario:
```rust
pub use database::PoolCandidate;
```

---

## PASO 8: Compilar y verificar errores

**Acción:** Ejecutar:
```bash
cd MIG_SDK_EXPORT
cargo check
```

**Errores comunes y soluciones:**

1. **Error: `PoolCandidate` not found**
   - Verificar que `PoolCandidate` tenga `pub` en `database.rs`
   - Verificar que `database` esté exportado en `lib.rs`

2. **Error: `PoolInitialState` not found**
   - Verificar que `hot_pool_manager` esté importado correctamente
   - Verificar que `PoolInitialState` esté exportado en `hot_pool_manager.rs`

3. **Error: `chrono` not found**
   - Agregar `chrono` a `Cargo.toml` si no está:
   ```toml
   chrono = { version = "0.4", features = ["serde"] }
   ```

4. **Error: `timeout` not found**
   - Verificar que `tokio::time::timeout` esté importado

5. **Error: `Bytes` not found**
   - Verificar que `ethers::types::Bytes` esté importado

---

## PASO 9: Ejecutar y verificar logs

**Acción:** Ejecutar el background_discoverer:
```bash
cargo run --bin background_discoverer
```

**Verificar en los logs:**
1. ✅ "Graph weights updated" - confirma que el cálculo de pesos funciona
2. ✅ "Populating Hot Pool Manager from database..." - confirma que se intenta poblar
3. ✅ "Loaded X pool candidates from database" - confirma que se cargan candidatos
4. ✅ "Validating top X candidates..." - confirma que se validan pools
5. ✅ "Added X hot pools to HotPoolManager" - confirma que se agregaron pools
6. ✅ "Hot Pool Manager populated with X pools" - confirma éxito final

**Si ves errores:**
- Revisar los logs para identificar el paso que falla
- Verificar que la base de datos tenga pools con `weight >= 100000.0`
- Verificar que `graph_weights` tenga datos actualizados

---

## Resumen de cambios

1. ✅ Removido código de poblamiento durante `calculate_and_update_all_weights`
2. ✅ Agregada función `load_pool_candidates` en `database.rs`
3. ✅ Agregadas estructuras y funciones auxiliares en `background_discoverer.rs`
4. ✅ Agregada función `populate_hot_pool_manager_from_db`
5. ✅ Integrada función en el ciclo de actualización de pesos
6. ✅ Verificados exports y compilación

---

## Notas importantes

- **Separación de responsabilidades:** El cálculo de pesos y el poblamiento del Hot Pool Manager ahora están separados
- **Resiliencia:** Si el cálculo de pesos falla temporalmente, el Hot Pool Manager puede seguir usando pesos de BD
- **Validación:** Los pools se validan on-chain antes de agregarse al Hot Pool Manager
- **Performance:** Se validan en batches de 50 pools para optimizar RPC calls
- **Threshold:** Solo se agregan pools con `weight >= $100K` para mantener alta calidad

---

## Próximos pasos (opcional)

1. Ajustar `target_valid_pools` (actualmente 50) según necesidades
2. Ajustar `min_weight` (actualmente $100K) según necesidades
3. Agregar métricas para monitorear el poblamiento
4. Agregar retry logic si la validación falla
