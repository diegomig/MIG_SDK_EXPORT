# MIG Topology SDK - Pipeline Flow Diagram

## Diagrama de Flujo Completo (Texto)

```
═══════════════════════════════════════════════════════════════════════════
                    MIG TOPOLOGY SDK - PIPELINE COMPLETO
═══════════════════════════════════════════════════════════════════════════

┌─────────────────────────────────────────────────────────────────────────┐
│                        1. INICIALIZACIÓN                                │
└─────────────────────────────────────────────────────────────────────────┘

    Settings::new()
        │
        ├──> Config.toml / Env Vars
        │
    RpcPool::new()
        │
        ├──> HTTP Providers (Alchemy, Infura, etc.)
        ├──> WebSocket Providers (opcional)
        ├──> Health Checks
        ├──> Circuit Breakers
        │
    Database::connect()
        │
        ├──> PostgreSQL Connection Pool
        ├──> Schema: mig_topology
        ├──> Initialize Tables (si no existen)
        │
    CacheManager::new()
        │
        ├──> Token Decimals Cache
        ├──> In-Memory Caches
        │
    PriceFeed::new()
        │
        ├──> Chainlink Oracles
        ├──> Price Cache
        ├──> TWAP Fallback (opcional)
        │
    FlightRecorder::new()
        │
        ├──> Event Channel (mpsc::UnboundedSender)
        ├──> Background Writer Task
        │
    DEX Adapters::new() [UniswapV2, UniswapV3, Camelot, etc.]
        │
        ├──> Factory Addresses
        ├──> Multicall Address
        ├──> Batch Size Configuration
        │
    PoolValidator::new()
        │
        ├──> Validation Rules
        ├──> Bytecode Verification
        │
    Orchestrator::new()
        │
        ├──> Inject: adapters, validator, db_pool, rpc_pool, price_feed
        ├──> Inject: cache_manager, settings
        ├──> with_flight_recorder() [opcional]
        ├──> with_block_stream() [opcional]
        │
    RedisManager::new() [opcional, requiere feature "redis"]
        │
        ├──> Redis Connection (ConnectionManager)
        ├──> Pool State Cache (TTL: 10s)
        ├──> Route Cache (TTL: 60s)
        │
    JitStateFetcher::new()
        │
        ├──> State Cache (DashMap)
        ├──> Multicall Address
        ├──> Batch Size Configuration
        ├──> with_flight_recorder() [opcional]
        ├──> with_redis() [opcional, requiere feature "redis"]
        │
    HotPoolManager::new()
        │
        ├──> V3 Hot Pools (DashMap<Address, V3PoolSnapshot>)
        ├──> V2 Hot Pools (DashMap<Address, V2PoolSnapshot>)
        ├──> Top-K Selection (configurable)
        ├──> Adaptive Refresh Rates
        ├──> StateQuality Tracking
        ├──> with_flight_recorder() [opcional]
        │
    GraphService::new()
        │
        ├──> Inject: rpc_pool, price_feed, db_pool
        ├──> Inject: jit_fetcher
        ├──> Graph Weights (DashMap<Address, f64>)
        ├──> with_flight_recorder() [opcional]
        ├──> with_hot_pool_manager() [opcional]
        │
    BlockStream::new()
        │
        ├──> Broadcast Channel (tokio::sync::broadcast)
        ├──> Multi-Subscriber Support
        ├──> with_redis() [opcional, Redis pub/sub para multi-process]


┌─────────────────────────────────────────────────────────────────────────┐
│                        2. DISCOVERY CYCLE                                │
│                    orchestrator.run_discovery_cycle()                   │
└─────────────────────────────────────────────────────────────────────────┘

    FLIGHT RECORDER: record_phase_start!("discovery_cycle")
    
    FOR EACH DEX Adapter:
        │
        ├──> Get Current Block (via BlockNumberCache o RpcPool)
        │
        ├──> Get/Create DEX State from DB (last_processed_block, mode)
        │
        ├──> Determine Block Range:
        │       └──> Reverse Sync: from = last_processed - initial_sync_blocks
        │                         to = last_processed
        │
        ├──> IF Curve (static registry):
        │       │
        │       ├──> adapter.discover_pools(current_block, current_block, ...)
        │       │       └──> Query MetaRegistry (static, no events)
        │       │
        │       ├──> validator.validate_all(pools_meta)
        │       │
        │       └──> database.upsert_pool() [for each validated pool]
        │       └──> CONTINUE (skip to next adapter)
        │
        ├──> ELSE (event-based discovery):
        │       │
        │       └──> Process Blocks in Chunks (chunk_size):
        │           │
        │           ├──> FLIGHT RECORDER: record_block_start!(chunk_start)
        │           │
        │           ├──> adapter.discover_pools(chunk_start, chunk_end, ...)
        │           │       │
        │           │       ├──> Query Events: PairCreated / PoolCreated
        │           │       │       └──> RPC: get_logs() [via RpcPool]
        │           │       │
        │           │       ├──> Parse Events → PoolMetadata[]
        │           │       │
        │           │       └──> Return: Vec<PoolMetadata>
        │           │
        │           ├──> FOR EACH Pool Metadata:
        │           │       │
        │           │       ├──> validator.validate(pool_meta)
        │           │       │       │
        │           │       │       ├──> Bytecode Verification
        │           │       │       │       └──> RPC: get_code() [via RpcPool]
        │           │       │       │
        │           │       │       ├──> Structural Validation
        │           │       │       │
        │           │       │       └──> Return: ValidationResult
        │           │       │
        │           │       ├──> IF Valid:
        │           │       │       │
        │           │       │       ├──> database.upsert_pool()
        │           │       │       │       │
        │           │       │       │       └──> FLIGHT RECORDER: Track DB latency
        │           │       │       │
        │           │       │       └──> valid_pools_meta.push(pool_meta)
        │           │       │
        │           │       └──> total_validated++
        │           │
        │           ├──> IF valid_pools_meta is NOT empty:
        │           │       │
        │           │       ├──> adapter.fetch_pool_state(&valid_pools_meta)
        │           │       │       │
        │           │       │       ├──> Batch Pool States via Multicall3
        │           │       │       │       │
        │           │       │       │       ├──> FOR V2 Pools:
        │           │       │       │       │       └──> Multicall: getReserves()
        │           │       │       │       │
        │           │       │       │       ├──> FOR V3 Pools:
        │           │       │       │       │       ├──> Multicall: slot0() → sqrt_price_x96, tick
        │           │       │       │       │       └──> Multicall: liquidity()
        │           │       │       │       │
        │           │       │       │       └──> RpcPool: acquire_multicall() [concurrency control]
        │           │       │       │
        │           │       │       └──> Return: Vec<Pool> [with current state]
        │           │       │
        │           │       ├──> Extract Unique Tokens from pools_with_state
        │           │       │
        │           │       ├──> price_feed.get_usd_prices_batch(&unique_tokens)
        │           │       │       │
        │           │       │       ├──> Multicall: Chainlink Oracle Queries
        │           │       │       │
        │           │       │       └──> Return: HashMap<Address, f64>
        │           │       │
        │           │       ├──> FOR EACH Pool in pools_with_state:
        │           │       │       │
        │           │       │       ├──> Calculate USD Value:
        │           │       │       │       ├──> V2: (reserve0 * price0) + (reserve1 * price1)
        │           │       │       │       └──> V3: liquidity * price (based on sqrt_price_x96)
        │           │       │       │
        │           │       │       ├──> Compare vs Threshold:
        │           │       │       │       ├──> V2: min_v2_reserve_usd
        │           │       │       │       └──> V3: min_v3_liquidity_usd
        │           │       │       │
        │           │       │       ├──> is_active = (usd_value >= threshold)
        │           │       │       │
        │           │       │       └──> database.set_pool_activity(pool_address, is_active)
        │           │       │               │
        │           │       │               └──> FLIGHT RECORDER: Track DB latency
        │           │       │
        │           │       └──> pools_processed_count = pools_with_state.len()
        │           │
        │           ├──> Calculate Shadow Gas Tracking:
        │           │       │
        │           │       ├──> IF pools_processed_count > 0:
        │           │       │       │
        │           │       │       ├──> num_individual_calls = pools_processed * avg_calls_per_pool (2)
        │           │       │       │
        │           │       │       ├──> num_multicall_batches = ceil(num_individual_calls / batch_size)
        │           │       │       │
        │           │       │       ├──> gas_individual_l1 = num_individual_calls * 21000
        │           │       │       │
        │           │       │       ├──> gas_individual_l2 = num_individual_calls * 1000
        │           │       │       │
        │           │       │       ├──> gas_multicall = (num_multicall_batches * 50000) + (num_individual_calls * 100)
        │           │       │       │
        │           │       │       ├──> gas_saved_l1 = gas_individual_l1 - gas_multicall
        │           │       │       │
        │           │       │       └──> gas_saved_l2 = gas_individual_l2 - gas_multicall
        │           │       │
        │           │       └──> ELSE: (None, None)
        │           │
        │           ├──> FLIGHT RECORDER: record_block_end!()
        │           │       │
        │           │       ├──> block: chunk_end
        │           │       ├──> duration_ms: chunk_start_time.elapsed()
        │           │       ├──> routes_generated: validated_count
        │           │       ├──> gas_saved_l1: Option<u64>
        │           │       └──> gas_saved_l2: Option<u64>
        │           │
        │           └──> database.set_dex_state(dex, last_processed_block)
        │                   │
        │                   └──> FLIGHT RECORDER: Track DB latency
        │
        ├──> Accumulate Metrics:
        │       ├──> total_discovered += pools_discovered
        │       ├──> total_validated += pools_validated
        │       ├──> total_inserted += pools_inserted
        │       └──> total_db_commit_time_ms += db_latency
        │
    └──> FLIGHT RECORDER: record_phase_end!("discovery_cycle")
            │
            ├──> duration_ms: discovery_start.elapsed()
            ├──> result: {
            │       ├──> pools_discovered: total_discovered
            │       ├──> pools_validated: total_validated
            │       ├──> pools_inserted: total_inserted
            │       ├──> db_commit_latency_ms: total_db_commit_time_ms
            │       ├──> rpc_success_rate: rpc_pool.get_resilience_stats().0
            │       └──> circuit_breaker_triggers: rpc_pool.get_resilience_stats().1
            │   }


┌─────────────────────────────────────────────────────────────────────────┐
│                      3. GRAPH WEIGHT UPDATES                            │
│            graph_service.calculate_and_update_all_weights()             │
└─────────────────────────────────────────────────────────────────────────┘

    FLIGHT RECORDER: record_phase_start!("graph_updates")
    
    Load Active Pools from Database:
        │
        ├──> database.load_active_pools()
        │       │
        │       └──> SQL: SELECT * FROM pools WHERE is_active = true AND is_valid = true
        │
        └──> Return: Vec<Pool> [up to DEFAULT_ACTIVE_POOL_LIMIT = 50000]
    
    IF pools is empty:
        └──> RETURN Ok(())
    
    Get RPC Provider:
        │
        └──> rpc_pool.get_next_provider()
                └──> Returns: (Provider, Permit) [with concurrency control]
    
    Get Current Block:
        │
        └──> provider.get_block_number()
    
    Batch Fetch Pool States via JIT State Fetcher:
        │
        ├──> Prepare PoolMetadata[] from pools
        │
        └──> jit_fetcher.fetch_current_states(&pool_metadata, current_block)
                │
                ├──> FLIGHT RECORDER: record_phase_start!("jit_fetch_internal")
                │
                ├──> Check Cache First (DashMap):
                │       │
                │       ├──> FOR EACH pool_metadata:
                │       │       │
                │       │       ├──> Check state_cache.get(pool_address)
                │       │       │       │
                │       │       │       ├──> IF cached state exists:
                │       │       │       │       │
                │       │       │       │       ├──> Check Cache Validity:
                │       │       │       │       │       ├──> Block Number Tolerance (configurable)
                │       │       │       │       │       ├──> TTL Check (configurable)
                │       │       │       │       │       └──> State Hash Match
                │       │       │       │       │
                │       │       │       │       └──> IF cache valid:
                │       │       │       │               └──> Use cached state (cache_hit++)
                │       │       │       │
                │       │       │       └──> ELSE:
                │       │       │               └──> Add to pools_to_fetch (cache_miss++)
                │       │       │
                │       │       └──> pools_to_fetch.push(pool_metadata)
                │
                ├──> IF pools_to_fetch is NOT empty:
                │       │
                │       ├──> Separate V2 and V3 Pools
                │       │
                │       ├──> Build Multicall Calls:
                │       │       │
                │       │       ├──> FOR V3 Pools:
                │       │       │       ├──> Call: slot0() → sqrt_price_x96, tick
                │       │       │       └──> Call: liquidity()
                │       │       │
                │       │       └──> FOR V2 Pools:
                │       │               └──> Call: getReserves() → reserve0, reserve1
                │       │
                │       ├──> Adaptive Batching:
                │       │       │
                │       │       ├──> Calculate Optimal Batch Size
                │       │       │
                │       │       ├──> Split Calls into Chunks
                │       │       │
                │       │       └──> Process Chunks in Parallel (max_parallelism)
                │       │
                │       ├──> rpc_pool.acquire_multicall()
                │       │       │
                │       │       ├──> Select Healthy Provider
                │       │       │
                │       │       ├──> Acquire Semaphore Permit (concurrency control)
                │       │       │
                │       │       └──> Execute Multicall3
                │       │               │
                │       │               └──> RPC: eth_call(multicall_aggregate, calls[])
                │       │
                │       ├──> Parse Multicall Results:
                │       │       │
                │       │       ├──> Decode Return Data
                │       │       │
                │       │       ├──> Map to Pool Addresses
                │       │       │
                │       │       └──> Build FreshPoolStates {
                │       │               ├──> v3_states: HashMap<Address, V3PoolState>
                │       │               └──> v2_states: HashMap<Address, (reserve0, reserve1)>
                │       │           }
                │       │
                │       └──> Update Cache:
                │               │
                │               ├──> FOR EACH fetched state:
                │               │       │
                │               │       ├──> Calculate State Hash
                │               │       │
                │               │       └──> state_cache.insert(pool_address, CachedPoolState {
                │               │               ├──> state: FreshPoolState
                │               │               ├──> cached_at_block: current_block
                │               │               ├──> cached_at_time: Instant::now()
                │               │               └──> touched: false
                │               │           })
                │
                ├──> FLIGHT RECORDER: record_phase_end!("jit_fetch_internal")
                │       │
                │       ├──> duration_ms: fetch_start.elapsed()
                │       ├──> result: {
                │       │       ├──> total_pools: pool_metadata.len()
                │       │       ├──> pools_to_fetch: pools_to_fetch.len()
                │       │       ├──> cache_hits: cached_count
                │       │       ├──> cache_misses: pools_to_fetch.len()
                │       │       ├──> cache_hit_rate: cache_hits / total_pools
                │       │       ├──> successful_calls: num_successful
                │       │       └──> total_calls: num_multicall_batches
                │       │   }
                │
                └──> Return: FreshPoolStates
    
    Calculate Weights for All Pools:
        │
        ├──> Get Token Prices:
        │       │
        │       └──> price_feed.get_usd_prices_batch(&all_tokens)
        │               └──> Chainlink Oracle Queries (batch via Multicall)
        │
        ├──> FOR EACH Pool:
        │       │
        │       ├──> Get Fresh State (from JIT Fetcher results or cache)
        │       │
        │       ├──> Calculate Liquidity USD:
        │       │       │
        │       │       ├──> V2 Pool:
        │       │       │       ├──> value0 = reserve0 * token0_price
        │       │       │       ├──> value1 = reserve1 * token1_price
        │       │       │       └──> weight = value0 + value1
        │       │       │
        │       │       └──> V3 Pool:
        │       │               ├──> Calculate amount0 and amount1 from sqrt_price_x96 and liquidity
        │       │               ├──> value0 = amount0 * token0_price
        │       │               ├──> value1 = amount1 * token1_price
        │       │               └──> weight = value0 + value1
        │       │
        │       └──> Update Graph Weight:
        │               │
        │               ├──> graph_weights.insert(pool_address, weight)
        │               │
        │               └──> database.upsert_graph_weight(pool_address, weight, current_block)
        │
        └──> total_processed++, total_updated++
    
    FLIGHT RECORDER: record_phase_end!("graph_updates")
        │
        ├──> duration_ms: graph_update_start.elapsed()
        ├──> state_staleness_ms: duration_ms [proxy metric]
        └──> result: {
                ├──> pools_processed: total_processed
                ├──> pools_updated: total_updated
                └──> state_staleness_ms: duration_ms
            }


┌─────────────────────────────────────────────────────────────────────────┐
│                      4. FLIGHT RECORDER (Background)                     │
└─────────────────────────────────────────────────────────────────────────┘

    Background Writer Task (spawned at initialization):
        │
        └──> flight_recorder_writer(event_rx, output_file)
                │
                ├──> Create Output File (JSON Lines format)
                │
                └──> LOOP:
                        │
                        ├──> event_rx.recv() → FlightEvent
                        │
                        ├──> Serialize Event to JSON
                        │
                        ├──> Write JSON Line to File
                        │
                        ├──> Flush Every 10 Events (or 100 events)
                        │
                        └──> CONTINUE


┌─────────────────────────────────────────────────────────────────────────┐
│                      5. RPC POOL MANAGEMENT                              │
└─────────────────────────────────────────────────────────────────────────┘

    RpcPool Operations:
        │
        ├──> get_next_provider()
        │       │
        │       ├──> Select Healthy Provider (Round-Robin + Health Check)
        │       │
        │       ├──> Acquire Semaphore Permit (concurrency control)
        │       │
        │       ├──> FLIGHT RECORDER: record_decision!("provider_selected")
        │       │
        │       └──> Return: (Provider, Permit)
        │
        ├──> acquire_multicall()
        │       │
        │       ├──> Select Healthy Provider
        │       │
        │       ├──> Acquire Semaphore Permit
        │       │
        │       ├──> Execute Multicall3
        │       │
        │       ├──> IF Success:
        │       │       └──> provider_status.success_count++
        │       │
        │       └──> IF Failure:
        │               │
        │               ├──> provider_status.failure_count++
        │               │
        │               └──> Circuit Breaker Logic:
        │                       │
        │                       ├──> IF failure_rate > threshold:
        │                       │       └──> circuit_breaker.state = Open
        │                       │
        │                       └──> Retry with Next Provider
        │
        └──> get_resilience_stats()
                │
                ├──> Aggregate success_count and failure_count across all providers
                │
                ├──> Calculate rpc_success_rate = total_success / (total_success + total_failures)
                │
                ├──> Count circuit_breaker_triggers (providers with state == Open)
                │
                └──> Return: (rpc_success_rate, circuit_breaker_triggers)


┌─────────────────────────────────────────────────────────────────────────┐
│                      6. COMPLETE SDK CYCLE                               │
│                    (benchmark_metrics.rs example)                        │
└─────────────────────────────────────────────────────────────────────────┘

    FOR cycle in 1..num_cycles:
        │
        ├──> Step 1: Discovery Cycle
        │       │
        │       └──> orchestrator.run_discovery_cycle()
        │               │
        │               └──> [See Section 2: Discovery Cycle]
        │
        ├──> Step 2: Graph Weight Updates
        │       │
        │       └──> graph_service.calculate_and_update_all_weights()
        │               │
        │               └──> [See Section 3: Graph Weight Updates]
        │
        └──> Record Cycle Duration
    
    Analyze Flight Recorder Events:
        │
        ├──> Parse JSON Lines File
        │
        ├──> Aggregate Metrics:
        │       ├──> BlockStart/BlockEnd Events → blocks_processed
        │       ├──> PhaseEnd Events → phase_durations, phase_results
        │       ├──> RpcCall Events → rpc_latencies, rpc_success_rate
        │       └──> Decision Events → provider_selections
        │
        └──> Generate Benchmark Report


═══════════════════════════════════════════════════════════════════════════
                            FLUJO DE DATOS PRINCIPAL
═══════════════════════════════════════════════════════════════════════════

    Blockchain (Arbitrum One)
        │
        ├──> Events (PairCreated, PoolCreated)
        │       └──> [via RpcPool] → DEX Adapters
        │
        ├──> Pool State (reserves, liquidity, sqrt_price_x96)
        │       └──> [via RpcPool + Multicall3] → JIT State Fetcher
        │
        └──> Token Prices (Chainlink Oracles)
                └──> [via RpcPool + Multicall3] → Price Feed
    
    DEX Adapters
        │
        ├──> discover_pools() → Vec<PoolMetadata>
        │       └──> Orchestrator
        │
        └──> fetch_pool_state() → Vec<Pool>
                └──> Orchestrator
    
    Orchestrator
        │
        ├──> Validated Pools → Database (PostgreSQL)
        │       └──> Schema: mig_topology.pools
        │
        ├──> Pool Activity Status → Database
        │       └──> Schema: mig_topology.pools (is_active)
        │
        └──> DEX State → Database
                └──> Schema: mig_topology.dex_state
    
    Graph Service
        │
        ├──> Active Pools ← Database
        │
        ├──> Fresh Pool States ← JIT State Fetcher
        │
        ├──> Token Prices ← Price Feed
        │
        ├──> Calculated Weights → Database
        │       └──> Schema: mig_topology.graph_weights
        │
        └──> Calculated Weights → In-Memory Graph (DashMap)
    
    Flight Recorder
        │
        ├──> Events from All Components
        │       ├──> BlockStart/BlockEnd (Orchestrator)
        │       ├──> PhaseStart/PhaseEnd (Orchestrator, GraphService, JIT Fetcher)
        │       ├──> RpcCall (RpcPool, Price Feed, JIT Fetcher)
        │       └──> Decision (RpcPool)
        │
        └──> JSON Lines File (benchmarks/flight_recorder_*.jsonl)


═══════════════════════════════════════════════════════════════════════════
                            COMPONENTES CLAVE
═══════════════════════════════════════════════════════════════════════════

    Orchestrator:
        - Coordina discovery, validation, persistence
        - Gestiona estado por DEX (last_processed_block)
        - Procesa bloques en chunks (reverse sync)
        - Calcula métricas: gas_saved, db_latency
    
    GraphService:
        - Mantiene grafo de liquidez ponderado
        - Calcula weights basados en reserves y precios USD
        - Actualiza weights en DB e in-memory
        - Usa JIT State Fetcher para estados frescos
        - Integración opcional con Hot Pool Manager
    
    JIT State Fetcher:
        - Fetching on-demand de estados de pools
        - Cache agresivo con invalidación por hash
        - Batching adaptativo con Multicall3
        - Fast path para pools no-touched
        - Backend opcional Redis para cache distribuido
    
    Hot Pool Manager:
        - Cache in-memory de top-K pools (por peso)
        - Adaptive refresh rates (warm/cold pools)
        - StateQuality tracking (Fresh/Stale/Corrupt)
        - Integración con GraphService para weight updates
    
    BlockStream:
        - Broadcast channel (tokio::sync::broadcast) para in-process
        - Opcional Redis pub/sub para multi-process coordination
        - Permite múltiples suscriptores del mismo stream de bloques
    
    Redis Manager:
        - Cache distribuido de estados de pools (TTL: 10s)
        - Route cache (TTL: 60s)
        - Pub/sub backend para BlockStream
        - Requiere feature flag "redis"
    
    RpcPool:
        - Pool de providers HTTP/WebSocket
        - Health checks y circuit breakers
        - Concurrency control (semáforos)
        - Load balancing y retry logic
    
    Price Feed:
        - Integración Chainlink Oracles
        - Batch queries via Multicall3
        - Cache de precios con TTL
        - TWAP fallback (opcional)
    
    Flight Recorder:
        - Sistema de eventos asíncrono (mpsc::UnboundedSender)
        - Writer task en background
        - Overhead mínimo (<1% CPU)
        - Formato JSON Lines para análisis posterior


═══════════════════════════════════════════════════════════════════════════
                            MÉTRICAS CAPTURADAS
═══════════════════════════════════════════════════════════════════════════

    BlockEnd Events:
        - gas_saved_l1: Gas ahorrado (L1 estimate)
        - gas_saved_l2: Gas ahorrado (L2 estimate)
        - duration_ms: Tiempo de procesamiento del bloque
    
    PhaseEnd Events:
        - discovery_cycle:
            * pools_discovered, pools_validated, pools_inserted
            * db_commit_latency_ms
            * rpc_success_rate
            * circuit_breaker_triggers
        - graph_updates:
            * pools_processed, pools_updated
            * state_staleness_ms
        - jit_fetch_internal:
            * cache_hit_rate, cache_hits, cache_misses
            * successful_calls, total_calls
    
    RpcCall Events:
        - endpoint, method
        - duration_ms, success
        - payload_size_bytes
        - pools_requested, pools_returned
    
    Decision Events:
        - provider_selected
        - circuit_breaker_opened
        - etc.


═══════════════════════════════════════════════════════════════════════════
                            OPTIMIZACIONES CLAVE
═══════════════════════════════════════════════════════════════════════════

    1. Multicall3 Batching:
       - Agrupa múltiples eth_call en un solo multicall
       - Reduce RPC calls en ~80%
       - Calcula gas_saved automáticamente
    
    2. JIT State Fetching con Cache:
       - Solo fetchea pools que cambiaron (hash-based invalidation)
       - Cache hit rate objetivo: >80%
       - Fast path para pools no-touched
    
    3. Parallel Processing:
       - Chunks procesados en paralelo (adaptativo)
       - Max parallelism configurable
       - Concurrency control via semáforos
    
    4. Price Feed Batching:
       - Batch de tokens únicos antes de query
       - Un solo multicall para todos los precios
    
    5. Database Optimization:
       - UPSERT en lugar de INSERT + UPDATE
       - Batch operations donde sea posible
       - Connection pooling
    
    6. RPC Pool:
       - Health checks automáticos
       - Circuit breakers
       - Load balancing
       - Automatic failover


═══════════════════════════════════════════════════════════════════════════
                            ESTRUCTURA DE DATOS
═══════════════════════════════════════════════════════════════════════════

    Database Tables:
        - mig_topology.pools: Pool metadata (address, dex, tokens, fee, etc.)
        - mig_topology.graph_weights: Calculated weights (pool_address, weight)
        - mig_topology.dex_state: Discovery state (dex, last_processed_block, mode)
        - mig_topology.pool_state_snapshots: Historical snapshots
        - mig_topology.tokens: Token metadata (address, symbol, decimals)
        - mig_topology.token_relations: Token wrap/bridge relations
        - mig_topology.pool_statistics: Pool analytics (TVL, volatility)
        - mig_topology.dex_statistics: DEX-level statistics
    
    In-Memory Structures:
        - DashMap<Address, f64>: Graph weights (lock-free reads)
        - DashMap<Address, CachedPoolState>: JIT State Cache
        - DashMap<Address, u8>: Token decimals cache
        - Arc<RpcPool>: Shared RPC provider pool
        - Arc<FlightRecorder>: Shared event recorder


═══════════════════════════════════════════════════════════════════════════
                            FLUJO COMPLETO EJEMPLO
═══════════════════════════════════════════════════════════════════════════

    1. Inicialización:
       Settings → RpcPool → Database → CacheManager → PriceFeed
       → FlightRecorder → RedisManager [opcional] → DEX Adapters
       → Validator → Orchestrator → JitStateFetcher [con Redis opcional]
       → HotPoolManager → GraphService [con HotPoolManager]
       → BlockStream [con Redis pub/sub opcional]
    
    2. Discovery Cycle:
       Orchestrator → [FOR EACH DEX]
                      → Get Block Range
                      → Process Chunks
                         → Discover Pools (RPC: get_logs)
                         → Validate Pools (RPC: get_code)
                         → Fetch States (RPC: Multicall3)
                         → Get Prices (RPC: Multicall3)
                         → Update Activity (DB)
                         → Record BlockEnd (Flight Recorder)
                      → Record PhaseEnd (Flight Recorder)
    
    3. Graph Updates:
       GraphService → Load Active Pools (DB)
                     → Fetch States via JIT Fetcher
                        → Check Cache
                        → Multicall3 for missing states
                        → Update Cache
                     → Calculate Weights (Prices + States)
                     → Update Graph (DB + In-Memory)
                     → Record PhaseEnd (Flight Recorder)
    
    4. Background Tasks:
       Flight Recorder Writer → Events → JSON Lines File
    
    5. Metrics Analysis:
       Parse Flight Recorder File → Aggregate → Benchmark Report


═══════════════════════════════════════════════════════════════════════════
```
