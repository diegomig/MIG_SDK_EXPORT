# ========================================
# Script de inicialización de base de datos (PowerShell)
# Espera a que PostgreSQL esté listo y ejecuta migraciones
# ========================================

param(
    [string]$PostgresHost = "localhost",
    [int]$PostgresPort = 5432,
    [string]$PostgresUser = "mig_topology_user",
    [string]$PostgresDb = "mig_topology",
    [string]$PostgresPassword = "mig_topology_pass",
    [string]$MigrationsDir = "..\migrations"
)

$ErrorActionPreference = "Stop"

Write-Host "🔍 Waiting for PostgreSQL to be ready..." -ForegroundColor Cyan

# Esperar a que PostgreSQL esté listo (máximo 60 intentos, 2 segundos cada uno = 2 minutos)
$MaxAttempts = 60
$Attempt = 0
$IsReady = $false

while ($Attempt -lt $MaxAttempts) {
    try {
        $env:PGPASSWORD = $PostgresPassword
        $result = & psql -h $PostgresHost -p $PostgresPort -U $PostgresUser -d $PostgresDb -c "SELECT 1;" 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Host "✅ PostgreSQL is ready!" -ForegroundColor Green
            $IsReady = $true
            break
        }
    } catch {
        # Continuar intentando
    }
    
    $Attempt++
    Write-Host "⏳ Attempt $Attempt/$MaxAttempts : PostgreSQL not ready yet, waiting 2 seconds..." -ForegroundColor Yellow
    Start-Sleep -Seconds 2
}

if (-not $IsReady) {
    Write-Host "❌ Error: PostgreSQL did not become ready after $MaxAttempts attempts" -ForegroundColor Red
    exit 1
}

Write-Host "📝 Executing database migrations..." -ForegroundColor Cyan

# Ejecutar migraciones SQL en orden
if (Test-Path $MigrationsDir) {
    $migrationFiles = Get-ChildItem -Path $MigrationsDir -Filter "*.sql" | Sort-Object Name
    
    foreach ($migrationFile in $migrationFiles) {
        Write-Host "  → Running migration: $($migrationFile.Name)" -ForegroundColor Cyan
        $env:PGPASSWORD = $PostgresPassword
        $result = & psql -h $PostgresHost -p $PostgresPort -U $PostgresUser -d $PostgresDb -f $migrationFile.FullName 2>&1
        
        if ($LASTEXITCODE -eq 0) {
            Write-Host "    ✅ Migration completed successfully" -ForegroundColor Green
        } else {
            Write-Host "    ❌ Migration failed" -ForegroundColor Red
            Write-Host $result
            exit 1
        }
    }
} else {
    Write-Host "⚠️  Warning: Migrations directory '$MigrationsDir' not found" -ForegroundColor Yellow
}

Write-Host "✅ Database initialization complete!" -ForegroundColor Green
