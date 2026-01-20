# Flight Recorder - Event Capture System

## Overview

The Flight Recorder is a lightweight event capture system designed for post-mortem analysis and public observability. It provides detailed event logging with minimal performance overhead (<1% CPU, ~10MB RAM per minute).

## Purpose

The Flight Recorder enables:

- **Post-Mortem Analysis**: Detailed event logs for debugging issues after they occur
- **Performance Analysis**: Latency and timing data for optimization
- **Public Observability**: Network status reports and system health metrics
- **Debugging**: Trace execution flow through complex async operations

## Architecture

### Event-Driven Design

The Flight Recorder uses an asynchronous, non-blocking design:

```rust
FlightRecorder (enabled/disabled)
    ↓
Event Channel (mpsc::UnboundedSender)
    ↓
Writer Task (async file writer)
    ↓
JSON Lines File (one event per line)
```

### Performance Characteristics

- **CPU Overhead**: <1% when enabled
- **Memory Usage**: ~10MB per minute of recording
- **Latency Impact**: <1ms per event (async, non-blocking)
- **Disk Usage**: ~1-2MB per minute (compressed JSON)

## Event Types

The Flight Recorder captures the following event types:

### BlockStart

Records the start of block processing:

```json
{
  "type": "BlockStart",
  "ts": 12345,
  "block": 200000000
}
```

### BlockEnd

Records the completion of block processing with statistics:

```json
{
  "type": "BlockEnd",
  "ts": 12500,
  "block": 200000000,
  "duration_ms": 155,
  "routes_generated": 42,
  "routes_filtered": 8,
  "opportunities": 0
}
```

### PhaseStart / PhaseEnd

Records the start and end of execution phases:

```json
{
  "type": "PhaseStart",
  "ts": 12350,
  "phase": "jit_fetch",
  "metadata": {
    "pools_requested": 10,
    "block": 200000000
  },
  "block": 200000000
}
```

```json
{
  "type": "PhaseEnd",
  "ts": 12380,
  "phase": "jit_fetch",
  "duration_ms": 30,
  "result": {
    "pools_fetched": 10,
    "cache_hits": 8,
    "cache_misses": 2
  },
  "block": 200000000
}
```

### Decision

Records critical decision points:

```json
{
  "type": "Decision",
  "ts": 12360,
  "component": "validator",
  "action": "filter",
  "reason": "insufficient_liquidity",
  "context": {
    "pool_address": "0x...",
    "liquidity_usd": 500.0,
    "min_threshold": 1000.0
  },
  "block": 200000000
}
```

### RpcCall

Records RPC call details:

```json
{
  "type": "RpcCall",
  "ts": 12370,
  "endpoint": "eth_call",
  "method": "getReserves",
  "duration_ms": 15,
  "success": true,
  "block": 200000000,
  "payload_size_bytes": 128,
  "pools_requested": 10,
  "pools_returned": 10
}
```

### Error

Records error events:

```json
{
  "type": "Error",
  "ts": 12375,
  "component": "rpc_pool",
  "error_type": "rpc_failure",
  "message": "Connection timeout",
  "context": {
    "provider": "https://...",
    "retry_count": 3
  },
  "block": 200000000
}
```

### BlockSkipped / BlockGap

Records block processing anomalies:

```json
{
  "type": "BlockSkipped",
  "ts": 12400,
  "block": 200000001,
  "reason": "duplicate",
  "last_processed_block": 200000000,
  "gap_blocks": 1
}
```

## Usage

### Enabling the Flight Recorder

```rust
use mig_topology_sdk::flight_recorder::FlightRecorder;

// Create recorder
let (recorder, event_rx) = FlightRecorder::new();

// Enable recording
recorder.enable();

// Spawn writer task
tokio::spawn(async move {
    flight_recorder_writer(
        event_rx,
        "flight_recorder.jsonl".to_string(),
    ).await
});

// Pass recorder to components
let orchestrator = Orchestrator::new(...)
    .with_flight_recorder(Arc::new(recorder));
```

### Recording Events

Use the provided macros for convenient event recording:

```rust
use mig_topology_sdk::{record_phase_start, record_phase_end, record_rpc_call};

// Record phase start
record_phase_start!(
    &flight_recorder,
    "price_fetch",
    json!({"tokens": 10}),
    Some(current_block)
);

// Record phase end
record_phase_end!(
    &flight_recorder,
    "price_fetch",
    start_time,
    json!({"prices_fetched": 10}),
    Some(current_block)
);

// Record RPC call
record_rpc_call!(
    &flight_recorder,
    "eth_call",
    "getReserves",
    start_time,
    true
);
```

### Disabling the Flight Recorder

```rust
recorder.disable();
```

When disabled, the Flight Recorder has zero overhead (early return in `record()` method).

## What It Records

The Flight Recorder captures:

### ✅ System Events
- Block processing start/end
- Phase execution (discovery, validation, graph updates)
- RPC call timing and success/failure
- Error events with context

### ✅ Performance Metrics
- Latency measurements (phase duration, RPC call duration)
- Throughput metrics (pools processed, routes generated)
- Cache statistics (hit/miss rates)

### ✅ Decision Points
- Pool validation decisions (accept/reject with reason)
- Filtering decisions (liquidity, quality thresholds)
- Coordination events (component synchronization)

### ✅ Network Status
- RPC provider health
- Block processing gaps
- Error rates and types

## What It Does NOT Record

**For privacy and security, the Flight Recorder explicitly does NOT record:**

### ❌ Private Data
- **Private keys**: Never recorded (SDK is read-only)
- **Wallet addresses**: User wallet addresses are not recorded
- **Transaction signatures**: No transaction signing data
- **API keys**: RPC provider API keys are not logged

### ❌ Sensitive Configuration
- **Database credentials**: Connection strings are not logged
- **Redis passwords**: Authentication tokens are not recorded
- **Internal IPs**: Network configuration details are excluded

### ❌ Trading Data
- **Profit calculations**: Trading-specific profit metrics are not recorded
- **Opportunity details**: Specific arbitrage opportunities are not logged
- **Execution strategies**: Trading execution logic is not captured

### ❌ User Data
- **Personal information**: No user-identifiable information
- **Usage patterns**: Individual usage patterns are not tracked
- **Query parameters**: User-specific query parameters are not logged

### ❌ Competitive Information
- **Advanced algorithms**: Proprietary optimization algorithms are not logged
- **Performance tricks**: Competitive performance optimizations are excluded
- **Internal metrics**: Internal-only performance metrics are not recorded

## Output Format

### JSON Lines Format

Events are written in JSON Lines format (one JSON object per line):

```
{"type":"BlockStart","ts":12345,"block":200000000}
{"type":"PhaseStart","ts":12350,"phase":"jit_fetch","metadata":{},"block":200000000}
{"type":"RpcCall","ts":12370,"endpoint":"eth_call","method":"getReserves","duration_ms":15,"success":true,"block":200000000}
{"type":"PhaseEnd","ts":12380,"phase":"jit_fetch","duration_ms":30,"result":{},"block":200000000}
{"type":"BlockEnd","ts":12500,"block":200000000,"duration_ms":155,"routes_generated":42,"routes_filtered":8,"opportunities":0}
```

### File Naming

Default output file: `flight_recorder.jsonl`

Custom file path can be specified:

```rust
flight_recorder_writer(event_rx, "logs/recording_20240102.jsonl".to_string()).await
```

## Analysis Tools

### Basic Analysis

```bash
# Count events by type
jq -r '.type' flight_recorder.jsonl | sort | uniq -c

# Filter errors
jq 'select(.type == "Error")' flight_recorder.jsonl

# Calculate average RPC latency
jq -r 'select(.type == "RpcCall") | .duration_ms' flight_recorder.jsonl | awk '{sum+=$1; count++} END {print sum/count}'
```

### Performance Analysis

```bash
# Phase duration statistics
jq -r 'select(.type == "PhaseEnd") | "\(.phase)\t\(.duration_ms)"' flight_recorder.jsonl | \
  awk '{phase=$1; dur=$2; sum[phase]+=dur; count[phase]++} END {for (p in sum) print p, sum[p]/count[p]}'
```

### Network Status Report

The Flight Recorder enables generation of public "Network Status Reports":

```rust
// Example: Generate network status report
fn generate_network_status_report(events: Vec<FlightEvent>) -> NetworkStatus {
    NetworkStatus {
        total_blocks_processed: count_block_events(events),
        average_block_latency: calculate_avg_latency(events),
        rpc_success_rate: calculate_rpc_success_rate(events),
        error_rate: calculate_error_rate(events),
        // ... other public metrics
    }
}
```

## Best Practices

### When to Enable

- **Debugging**: Enable when investigating specific issues
- **Performance Analysis**: Enable for performance profiling sessions
- **Network Monitoring**: Enable for generating public network status reports

### When NOT to Enable

- **Production (High Load)**: Disable in production to minimize overhead
- **Long-Running Processes**: Use with caution for extended periods (disk space)
- **Sensitive Operations**: Disable when processing sensitive data

### Disk Space Management

- **Rotation**: Implement log rotation for long-running processes
- **Compression**: Compress old logs to save space
- **Retention**: Set retention policies (e.g., keep last 7 days)

## Configuration

### Environment Variables

```bash
# Enable flight recorder
FLIGHT_RECORDER_ENABLED=true

# Output file path
FLIGHT_RECORDER_OUTPUT=logs/flight_recorder.jsonl
```

### Programmatic Control

```rust
// Enable/disable at runtime
recorder.enable();
recorder.disable();

// Check status
if recorder.is_enabled() {
    // Record events
}
```

## Privacy & Security

### Data Minimization

The Flight Recorder follows the principle of data minimization:
- Only records essential operational data
- Excludes sensitive and private information
- Aggregates statistics rather than raw data where possible

### Public Observability

Events recorded by the Flight Recorder are suitable for:
- Public network status reports
- Performance benchmarking
- System health monitoring

Events are **NOT** suitable for:
- User tracking
- Competitive analysis
- Sensitive data exposure

## Future Enhancements

See `docs/ROADMAP.md` for planned enhancements:

- **Filtered Recording**: Record only specific event types
- **Sampling**: Record only a percentage of events
- **Remote Logging**: Send events to remote logging service
- **Real-Time Analysis**: Stream events for real-time monitoring

## Example: Network Status Report

The Flight Recorder enables generation of public network status reports:

```json
{
  "timestamp": "2024-01-02T12:00:00Z",
  "period": "1h",
  "blocks_processed": 3600,
  "average_block_latency_ms": 150,
  "rpc_success_rate": 0.998,
  "error_rate": 0.002,
  "cache_hit_rate": 0.82,
  "pools_discovered": 1200,
  "pools_validated": 1150,
  "validation_success_rate": 0.958
}
```

This report can be published publicly without exposing sensitive information.

---

**Note**: The Flight Recorder is designed for observability and debugging. It does not record private keys, user data, or competitive information. All recorded events are suitable for public network status reports.

