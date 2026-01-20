# Quick Benchmark Validation Script (PowerShell)
# 
# This script runs the SDK with shortened intervals to generate benchmark metrics quickly
# for grant applications. It modifies intervals temporarily for faster execution.
#
# Usage:
#   .\scripts\run_quick_benchmark.ps1
#
# Requirements:
#   - DATABASE_URL, REDIS_URL, SDK_RPC_HTTP_URLS environment variables set
#   - PostgreSQL and Redis running (via docker-compose)
#   - Rust toolchain installed

Write-Host "🚀 Quick Benchmark Validation Script" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════════"
Write-Host ""

# Load environment variables from .env file
$envFile = "docker_infrastructure\.env"
if (Test-Path $envFile) {
    Write-Host "📋 Loading environment variables from docker_infrastructure\.env..." -ForegroundColor Yellow
    Get-Content $envFile | Where-Object { $_ -match '^(DATABASE_URL|REDIS_URL|SDK_RPC)' -and $_ -notmatch '^#' } | ForEach-Object {
        $key, $value = $_ -split '=', 2
        [Environment]::SetEnvironmentVariable($key, $value, "Process")
    }
}

# Check required environment variables
if (-not $env:DATABASE_URL) {
    Write-Host "❌ Error: DATABASE_URL environment variable is not set" -ForegroundColor Red
    exit 1
}

if (-not $env:SDK_RPC_HTTP_URLS) {
    Write-Host "❌ Error: SDK_RPC_HTTP_URLS environment variable is not set" -ForegroundColor Red
    exit 1
}

Write-Host "✅ Environment variables loaded" -ForegroundColor Green
Write-Host ""

# Run benchmark
Write-Host "📊 Running benchmark metrics collection..." -ForegroundColor Cyan
Write-Host "   This will execute multiple SDK cycles and collect real metrics" -ForegroundColor Gray
Write-Host ""

cargo run --example benchmark_metrics

Write-Host ""
Write-Host "✅ Benchmark validation complete!" -ForegroundColor Green
Write-Host "📁 Check benchmarks\ directory for generated reports" -ForegroundColor Cyan
Write-Host "📁 Check logs\ directory for Flight Recorder events" -ForegroundColor Cyan
