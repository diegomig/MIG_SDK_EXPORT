#!/bin/bash
cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT

echo "🧹 Cleaning old files..."
rm -f benchmarks/flight_recorder_*.jsonl logs/flight_recorder_*.jsonl 2>/dev/null

echo ""
echo "🚀 Step 1: Running benchmark_metrics (3 minutes)..."
echo "═══════════════════════════════════════════════════════════════════"
timeout 180 cargo run --example benchmark_metrics --features redis,observability 2>&1 | tee /tmp/benchmark_full.log | tail -20

echo ""
echo "📊 Step 2: Analyzing benchmark results..."
BENCH_FILE=$(ls -t benchmarks/flight_recorder_*.jsonl 2>/dev/null | head -1)
if [ -n "$BENCH_FILE" ]; then
    echo "Found: $BENCH_FILE"
    python3 analyze_flight_recorder.py
else
    echo "⚠️ No benchmark file found"
fi

echo ""
echo "🚀 Step 3: Running background_discoverer (2.5 minutes to capture initial hot refresh)..."
echo "═══════════════════════════════════════════════════════════════════"
# Source cargo env again in case it was lost
source ~/.cargo/env 2>/dev/null || true
timeout 150 cargo run --bin background_discoverer --features redis,observability 2>&1 | tee /tmp/discoverer_full.log | tail -30

echo ""
echo "📊 Step 4: Analyzing all results..."
python3 analyze_system_interactions.py

echo ""
echo "✅ Analysis complete!"
