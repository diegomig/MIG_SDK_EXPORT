# Script PowerShell para ejecutar la migración de datos desde Docker

Write-Host "🚀 Iniciando migración de datos desde Docker..." -ForegroundColor Cyan

# Verificar que los contenedores estén corriendo
Write-Host "🔍 Verificando contenedores Docker..." -ForegroundColor Yellow

$sourceContainer = docker ps --filter "name=arbitrage-postgres" --format "{{.Names}}"
$targetContainer = docker ps --filter "name=mig-topology-postgres" --format "{{.Names}}"

if (-not $sourceContainer) {
    Write-Host "❌ Error: El contenedor arbitrage-postgres no está corriendo" -ForegroundColor Red
    Write-Host "   Ejecuta: cd arbitrage-bot-v2 && docker compose up -d postgres" -ForegroundColor Yellow
    exit 1
}

if (-not $targetContainer) {
    Write-Host "❌ Error: El contenedor mig-topology-postgres no está corriendo" -ForegroundColor Red
    Write-Host "   Ejecuta: cd MIG_SDK_EXPORT/docker_infrastructure && docker compose up -d postgres" -ForegroundColor Yellow
    exit 1
}

Write-Host "✅ Contenedores verificados" -ForegroundColor Green

# Instalar dependencias si es necesario
try {
    python -c "import psycopg2" 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "psycopg2 not found"
    }
} catch {
    Write-Host "📦 Instalando psycopg2..." -ForegroundColor Yellow
    pip install psycopg2-binary
}

# Ejecutar el script de migración
Write-Host ""
Write-Host "📡 Ejecutando migración..." -ForegroundColor Cyan
$scriptPath = Join-Path $PSScriptRoot "migrate_db_data.py"
python $scriptPath

Write-Host ""
Write-Host "✅ Migración completada" -ForegroundColor Green
