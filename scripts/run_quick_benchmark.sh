#!/bin/bash
# Quick Benchmark Validation Script
# 
# This script runs the SDK with shortened intervals to generate benchmark metrics quickly
# for grant applications. It modifies intervals temporarily for faster execution.
#
# Usage:
#   ./scripts/run_quick_benchmark.sh
#
# Requirements:
#   - DATABASE_URL, REDIS_URL, SDK_RPC_HTTP_URLS environment variables set
#   - PostgreSQL and Redis running (via docker-compose)
#   - Rust toolchain installed

set -e

echo "🚀 Quick Benchmark Validation Script"
echo "═══════════════════════════════════════════════════════════════════"
echo ""

# Load environment variables
if [ -f "docker_infrastructure/.env" ]; then
    echo "📋 Loading environment variables from docker_infrastructure/.env..."
    export $(grep -v '^#' docker_infrastructure/.env | grep -E '^(DATABASE_URL|REDIS_URL|SDK_RPC)' | xargs)
fi

# Check required environment variables
if [ -z "$DATABASE_URL" ]; then
    echo "❌ Error: DATABASE_URL environment variable is not set"
    exit 1
fi

if [ -z "$SDK_RPC_HTTP_URLS" ]; then
    echo "❌ Error: SDK_RPC_HTTP_URLS environment variable is not set"
    exit 1
fi

echo "✅ Environment variables loaded"
echo ""

# Load Rust environment
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

# Run benchmark with shortened intervals
# The benchmark_metrics example executes cycles directly (no intervals), so it's already optimized
echo "📊 Running benchmark metrics collection..."
echo "   This will execute multiple SDK cycles and collect real metrics"
echo ""

cargo run --example benchmark_metrics

echo ""
echo "✅ Benchmark validation complete!"
echo "📁 Check benchmarks/ directory for generated reports"
echo "📁 Check logs/ directory for Flight Recorder events"
