#!/bin/bash
cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT

LATEST=$(ls -t benchmarks/flight_recorder_*.jsonl 2>/dev/null | head -1)

if [ -z "$LATEST" ]; then
    echo "❌ No flight recorder files found"
    exit 1
fi

echo "📊 Analyzing: $LATEST"
echo "═══════════════════════════════════════════════════════════════════"
echo ""

echo "📊 Event Types Distribution:"
cat "$LATEST" | jq -r '.type' | sort | uniq -c | sort -rn
echo ""

echo "📊 Weight Refresh Events:"
cat "$LATEST" | jq 'select(.phase == "weight_refresh_hot" or .phase == "weight_refresh_warm" or (.phase == "graph_updates" and (.metadata.mode == "full" or .result.mode == "full")))' | jq -s 'group_by(.phase) | map({phase: .[0].phase, count: length, avg_duration_ms: (map(.duration_ms // 0) | add / length)})'
echo ""

echo "📊 Graph Updates (All Modes):"
cat "$LATEST" | jq 'select(.phase == "graph_updates")' | jq -s 'group_by(.metadata.mode // .result.mode // "unknown") | map({mode: .[0].metadata.mode // .[0].result.mode // "unknown", count: length, avg_duration_ms: (map(.duration_ms // 0) | add / length), avg_pools: (map(.result.total_pools // .result.pools_updated // .metadata.total_pools // 0) | add / length)})'
echo ""

echo "📊 Cache Events:"
cat "$LATEST" | jq 'select(.type == "CacheEvent")' | jq -s 'group_by(.event_type) | map({event_type: .[0].event_type, count: length})'
echo ""

echo "📊 RPC Call Stats:"
cat "$LATEST" | jq 'select(.type == "RpcCall")' | jq -s '{total: length, successful: [.[] | select(.success == true)] | length, failed: [.[] | select(.success == false)] | length, avg_duration_ms: ([.[] | .duration_ms // 0] | add / length)}'
echo ""

echo "📊 Discovery Cycles:"
cat "$LATEST" | jq 'select(.phase == "discovery_cycle")' | jq -s '{count: length, avg_duration_ms: ([.[] | .duration_ms // 0] | add / length), avg_pools_discovered: ([.[] | .result.pools_discovered // 0] | add / length)}'
echo ""

echo "📊 Hot Pool Manager Updates:"
cat "$LATEST" | jq 'select(.phase == "hot_pool_manager_update_weights")' | jq -s '{count: length, avg_duration_ms: ([.[] | .duration_ms // 0] | add / length), avg_weights_count: ([.[] | .result.weights_count // 0] | add / length)}'
echo ""
