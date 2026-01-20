# Feature Flags

All feature flags are configured in `Config.toml` under `[features]` section.

## Available Flags

### `enable_websocket_blocks`

**Default**: `true`

Enables WebSocket subscription to `eth_subscribe("newHeads")` for real-time block number updates.

**Disable when**: WebSocket connections are unstable or not supported by RPC provider.

### `enable_polling_fallback`

**Default**: `true`

Enables polling fallback (1s interval) when WebSocket is disconnected >5 seconds.

**Disable when**: You want to fail fast on WebSocket disconnection (not recommended).

### `enable_event_indexing`

**Default**: `true`

Enables event indexing for gap detection. Stores all pool creation events in `mig_topology.event_index` table.

**Disable when**: You don't need historical event tracking or gap detection.

### `enable_price_fallback_chain`

**Default**: `true`

Enables price feed fallback chain: Chainlink → CoinGecko → Uniswap V3 TWAP.

**Disable when**: You only want Chainlink prices (faster, but less coverage).

### `enable_merkle_cache`

**Default**: `true`

Enables Merkle tree cache for JIT state fetcher with TTL diferenciado (30s for touched pools, 5min for others).

**Disable when**: You need always-fresh state (increases RPC calls significantly).

### `enable_streaming_multicall`

**Default**: `true`

Enables streaming multicall for incremental result processing.

**Disable when**: You prefer batch processing (current default behavior).

## Configuration Example

```toml
[features]
enable_websocket_blocks = true
enable_polling_fallback = true
enable_event_indexing = true
enable_price_fallback_chain = true
enable_merkle_cache = true
enable_streaming_multicall = true
```

## Disabling Features

To disable a feature, set it to `false`:

```toml
[features]
enable_websocket_blocks = false  # Disable WebSocket, use polling only
```

