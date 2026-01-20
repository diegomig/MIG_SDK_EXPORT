# Informe: Problema de Precios de Tokens en SDK

## Problema Identificado

El SDK solo devuelve precios para los 6 tokens cacheados y cuando debe consultar el resto devuelve cero (0.0). Esto causa que la mayoría de los pools tengan `weight=0` porque no se pueden calcular los precios de los tokens.

## Comparación: Bot Original vs SDK

### 1. Manejo de Tokens Faltantes

#### Bot Original (`arbitrage-bot-v2/routegen-rs/src/price_feeds.rs`)

**Líneas 550-562:**
```rust
// ✅ FASE 8: Verificar si hay tokens sin precio y retornar error explícito
let missing_tokens: Vec<Address> = tokens_to_fetch.iter()
    .filter(|t| !results.contains_key(t))
    .copied()
    .collect();

if !missing_tokens.is_empty() {
    return Err(anyhow::anyhow!(
        "FASE 8: Price feed failed for {} tokens: Chainlink, pool fallback, and hardcoded fallbacks all failed. Missing tokens: {:?}",
        missing_tokens.len(),
        missing_tokens
    ));
}
```

**Comportamiento:**
- Retorna **error explícito** cuando faltan tokens
- Fuerza al código llamador a manejar el caso de tokens faltantes
- No permite continuar con precios parciales sin manejo explícito

#### SDK (`MIG_SDK_EXPORT/src/price_feeds.rs`)

**Líneas 626-643:**
```rust
// ✅ FIX: Return partial results instead of error when some tokens fail
// This allows graph_service to use whatever prices were successfully fetched
let missing_tokens: Vec<Address> = tokens_to_fetch.iter()
    .filter(|t| !results.contains_key(t))
    .copied()
    .collect();

if !missing_tokens.is_empty() {
    // Log warning but don't fail - return partial results
    log::warn!(
        "⚠️ FASE 2.3: Price feed failed for {} tokens (out of {} requested): Chainlink, pool fallback, and hardcoded fallbacks all failed. Missing tokens: {:?}. Returning {} partial prices.",
        missing_tokens.len(),
        tokens_to_fetch.len(),
        missing_tokens,
        results.len()
    );
    // Continue and return partial results instead of error
}
```

**Comportamiento:**
- Retorna **resultados parciales** sin error
- Permite continuar con precios parciales
- **PROBLEMA**: Cuando `graph_service` consulta un precio que no está en `results`, obtiene `None` o `0.0`

### 2. Uso en GraphService

#### Bot Original

El bot original tiene múltiples mecanismos para manejar precios faltantes:

1. **Emergency Price Repair** (líneas 4123-4158 en `mvp_runner_refactored.rs`):
```rust
// ✅ Root fix (CU/s bounded): if we're missing prices for tokens used this block,
// trigger a background "repair" fetch from Chainlink with a longer timeout and patch SharedPriceCache.
let missing: Vec<Address> = token_prices.iter()
    .filter(|(t, p)| **p <= 0.0)
    .map(|(t, _)| *t)
    .collect();

if !missing.is_empty() {
    // Emergency fetch con timeout más largo
    match price_feed_arc
        .get_usd_prices_batch_with_chainlink_timeout(&missing, None, Duration::from_millis(1500))
        .await
    {
        Ok(repair_prices) => {
            for (token, price) in repair_prices {
                if price > 0.0 {
                    price_cache.set_price(token, price, 0);
                    token_prices.insert(token, price);
                }
            }
        }
        Err(_) => {}
    }
}
```

2. **Price Prefetch** (líneas 2117-2280):
   - Pre-fetch de precios antes de procesar bloques
   - Usa `SharedPriceCache` para obtener precios cacheados
   - Hace fetch de emergencia para tokens faltantes

3. **Warm-up de Precios** (líneas 3397-3461):
   - Warm-up de precios base antes de iniciar el bot
   - Validación de que todos los precios base estén disponibles

#### SDK

El SDK tiene un manejo más limitado:

1. **Uso de SharedPriceCache** (líneas 350-386 en `price_feeds.rs`):
   - Consulta `SharedPriceCache` para anchor tokens y tokens solicitados
   - Si no está disponible, solo usa Chainlink y pool fallback

2. **Falta de Emergency Repair**:
   - No hay mecanismo de "emergency repair" para tokens faltantes
   - No hay warm-up de precios base
   - No hay validación de precios antes de procesar

### 3. Diferencias Clave en el Flujo

#### Bot Original

```
1. Warm-up de precios base (startup)
2. Pre-fetch de precios antes de cada bloque
3. Consulta SharedPriceCache (rápido, 0-5ms)
4. Si faltan precios críticos → Emergency fetch con timeout largo (1500ms)
5. Si faltan precios entry → Emergency fetch con timeout largo (1500ms)
6. Si todo falla → Error explícito (no continúa con 0.0)
```

#### SDK

```
1. Consulta SharedPriceCache (si está disponible)
2. Consulta Chainlink (timeout: 150-500ms)
3. Pool fallback (si hay anchor tokens)
4. Hardcoded fallback (solo para stablecoins/WETH)
5. Si todo falla → Retorna resultados parciales (sin error)
6. graph_service obtiene 0.0 para tokens faltantes → weight=0
```

## Cambios Necesarios en el SDK

### Cambio 1: Agregar Emergency Price Repair

**Ubicación**: `MIG_SDK_EXPORT/src/graph_service.rs` o `MIG_SDK_EXPORT/src/orchestrator.rs`

**Implementación**:

```rust
// Después de obtener precios, verificar si faltan precios críticos
let missing_critical: Vec<Address> = prices_map.iter()
    .filter(|(_, price)| **price <= 0.0)
    .map(|(token, _)| *token)
    .copied()
    .collect();

if !missing_critical.is_empty() {
    // Emergency fetch con timeout más largo (1500ms)
    match self.price_feed
        .get_usd_prices_batch_with_chainlink_timeout(
            &missing_critical,
            None,
            Duration::from_millis(1500)
        )
        .await
    {
        Ok(repair_prices) => {
            for (token, price) in repair_prices {
                if price > 0.0 {
                    prices_map.insert(token, price);
                    // Actualizar SharedPriceCache si está disponible
                    if let Some(cache) = &self.shared_price_cache {
                        cache.set_price(token, price, current_block);
                    }
                }
            }
        }
        Err(e) => {
            warn!("⚠️ Emergency price repair failed: {}", e);
        }
    }
}
```

### Cambio 2: Agregar Warm-up de Precios Base

**Ubicación**: `MIG_SDK_EXPORT/src/orchestrator.rs` o `MIG_SDK_EXPORT/bin/background_discoverer.rs`

**Implementación**:

```rust
// Warm-up de precios base antes de iniciar
async fn warmup_base_prices(
    price_feed: &PriceFeed<Provider<Http>>,
    shared_cache: Option<&SharedPriceCache>,
    base_tokens: &[Address],
) -> Result<()> {
    info!("🔥 Warming up base prices for {} tokens...", base_tokens.len());
    
    // Intentar obtener precios con timeout largo
    match price_feed
        .get_usd_prices_batch_with_chainlink_timeout_and_cache(
            base_tokens,
            None,
            Duration::from_millis(2000),
            shared_cache
        )
        .await
    {
        Ok(prices) => {
            let valid_count = prices.values().filter(|&&p| p > 0.0).count();
            if valid_count < base_tokens.len() / 2 {
                return Err(anyhow::anyhow!(
                    "Only {} out of {} base prices available after warm-up",
                    valid_count,
                    base_tokens.len()
                ));
            }
            
            // Actualizar SharedPriceCache
            if let Some(cache) = shared_cache {
                cache.update_batch(prices.clone(), PriceSource::Chainlink);
            }
            
            info!("✅ Warm-up complete: {} valid prices", valid_count);
            Ok(())
        }
        Err(e) => {
            Err(anyhow::anyhow!("Warm-up failed: {}", e))
        }
    }
}
```

### Cambio 3: Mejorar Manejo de Resultados Parciales

**Ubicación**: `MIG_SDK_EXPORT/src/price_feeds.rs`

**Cambio**: En lugar de solo loggear warning, agregar información más detallada:

```rust
if !missing_tokens.is_empty() {
    // Log warning but don't fail - return partial results
    log::warn!(
        "⚠️ FASE 2.3: Price feed failed for {} tokens (out of {} requested): Chainlink, pool fallback, and hardcoded fallbacks all failed. Missing tokens: {:?}. Returning {} partial prices.",
        missing_tokens.len(),
        tokens_to_fetch.len(),
        missing_tokens.iter().take(10).collect::<Vec<_>>(), // Solo primeros 10 para no saturar logs
        results.len()
    );
    
    // ✅ NUEVO: Agregar métricas para monitoreo
    metrics::increment_counter_named("price_fetch_missing_tokens_total".to_string());
    metrics::histogram!("price_fetch_missing_tokens_count", missing_tokens.len() as f64);
    
    // Continue and return partial results instead of error
}
```

### Cambio 4: Validar Precios Antes de Usar en GraphService

**Ubicación**: `MIG_SDK_EXPORT/src/graph_service.rs`

**Implementación**:

```rust
// Después de obtener precios, validar que tenemos suficientes
if prices_map.len() < tokens_vec.len() / 10 {
    warn!("⚠️ Only loaded {} prices for {} tokens. Most pools will have weight = 0.", 
          prices_map.len(), tokens_vec.len());
    
    // ✅ NUEVO: Intentar emergency repair antes de continuar
    let missing_tokens: Vec<Address> = tokens_vec.iter()
        .filter(|t| !prices_map.contains_key(t) || prices_map.get(t).map(|&p| p <= 0.0).unwrap_or(true))
        .copied()
        .collect();
    
    if !missing_tokens.is_empty() && missing_tokens.len() < tokens_vec.len() / 2 {
        // Solo intentar repair si faltan menos del 50%
        match self.price_feed
            .get_usd_prices_batch_with_chainlink_timeout(
                &missing_tokens,
                None,
                Duration::from_millis(1500)
            )
            .await
        {
            Ok(repair_prices) => {
                for (token, price) in repair_prices {
                    if price > 0.0 {
                        prices_map.insert(token, price);
                    }
                }
                info!("✅ Emergency repair: recovered {} prices", repair_prices.len());
            }
            Err(e) => {
                warn!("⚠️ Emergency repair failed: {}", e);
            }
        }
    }
}
```

### Cambio 5: Inicializar SharedPriceCache con Precios Base

**Ubicación**: `MIG_SDK_EXPORT/bin/background_discoverer.rs` o donde se inicializa `BackgroundPriceUpdater`

**Implementación**:

```rust
// Al inicializar BackgroundPriceUpdater, asegurar que los anchor tokens estén en cache
let anchor_tokens = vec![
    // WETH, USDC, USDT, etc.
];

// Warm-up inicial de anchor tokens
match price_feed
    .get_usd_prices_batch_with_chainlink_timeout(
        &anchor_tokens,
        None,
        Duration::from_millis(2000)
    )
    .await
{
    Ok(prices) => {
        shared_price_cache.update_batch(prices, PriceSource::Chainlink);
        info!("✅ Initialized SharedPriceCache with {} anchor prices", prices.len());
    }
    Err(e) => {
        warn!("⚠️ Failed to initialize anchor prices: {}", e);
    }
}
```

## Resumen de Cambios

1. ✅ **Agregar Emergency Price Repair**: Mecanismo para recuperar precios faltantes con timeout largo
2. ✅ **Agregar Warm-up de Precios Base**: Validar que los precios base estén disponibles al inicio
3. ✅ **Mejorar Manejo de Resultados Parciales**: Agregar métricas y logging más detallado
4. ✅ **Validar Precios Antes de Usar**: Verificar que tenemos suficientes precios antes de calcular weights
5. ✅ **Inicializar SharedPriceCache**: Asegurar que los anchor tokens estén en cache desde el inicio

## Prioridad de Implementación

1. **P0 (Crítico)**: Cambio 4 (Validar Precios) + Cambio 1 (Emergency Repair)
2. **P1 (Importante)**: Cambio 5 (Inicializar SharedPriceCache)
3. **P2 (Mejora)**: Cambio 2 (Warm-up) + Cambio 3 (Métricas)

## Impacto Esperado

- **Antes**: Solo 6 tokens con precio → mayoría de pools con weight=0
- **Después**: Mecanismo de recuperación → más tokens con precio → más pools con weight>0
- **Métrica objetivo**: >80% de tokens solicitados con precio válido
