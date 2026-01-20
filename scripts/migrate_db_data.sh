#!/bin/bash
# Script para ejecutar la migración de datos desde Docker

set -e

echo "🚀 Iniciando migración de datos desde Docker..."

# Verificar que los contenedores estén corriendo
echo "🔍 Verificando contenedores Docker..."

if ! docker ps | grep -q "arbitrage-postgres"; then
    echo "❌ Error: El contenedor arbitrage-postgres no está corriendo"
    echo "   Ejecuta: cd arbitrage-bot-v2 && docker compose up -d postgres"
    exit 1
fi

if ! docker ps | grep -q "mig-topology-postgres"; then
    echo "❌ Error: El contenedor mig-topology-postgres no está corriendo"
    echo "   Ejecuta: cd MIG_SDK_EXPORT/docker_infrastructure && docker compose up -d postgres"
    exit 1
fi

echo "✅ Contenedores verificados"

# Instalar dependencias si es necesario
if ! python3 -c "import psycopg2" 2>/dev/null; then
    echo "📦 Instalando psycopg2..."
    pip3 install psycopg2-binary
fi

# Ejecutar el script de migración
echo ""
echo "📡 Ejecutando migración..."
python3 "$(dirname "$0")/migrate_db_data.py"

echo ""
echo "✅ Migración completada"
