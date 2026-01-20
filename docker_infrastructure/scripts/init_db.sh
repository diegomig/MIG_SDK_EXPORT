#!/bin/bash
# ========================================
# Script de inicialización de base de datos
# Espera a que PostgreSQL esté listo y ejecuta migraciones
# ========================================

set -e

# Variables de configuración
POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-mig_topology_user}"
POSTGRES_DB="${POSTGRES_DB:-mig_topology}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-mig_topology_pass}"
MIGRATIONS_DIR="${MIGRATIONS_DIR:-../migrations}"

echo "🔍 Waiting for PostgreSQL to be ready..."

# Esperar a que PostgreSQL esté listo (máximo 60 intentos, 2 segundos cada uno = 2 minutos)
MAX_ATTEMPTS=60
ATTEMPT=0

while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
    if PGPASSWORD="$POSTGRES_PASSWORD" psql -h "$POSTGRES_HOST" -p "$POSTGRES_PORT" -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT 1;" > /dev/null 2>&1; then
        echo "✅ PostgreSQL is ready!"
        break
    fi
    
    ATTEMPT=$((ATTEMPT + 1))
    echo "⏳ Attempt $ATTEMPT/$MAX_ATTEMPTS: PostgreSQL not ready yet, waiting 2 seconds..."
    sleep 2
done

if [ $ATTEMPT -eq $MAX_ATTEMPTS ]; then
    echo "❌ Error: PostgreSQL did not become ready after $MAX_ATTEMPTS attempts"
    exit 1
fi

echo "📝 Executing database migrations..."

# Ejecutar migraciones SQL en orden
if [ -d "$MIGRATIONS_DIR" ]; then
    for migration_file in "$MIGRATIONS_DIR"/*.sql; do
        if [ -f "$migration_file" ]; then
            echo "  → Running migration: $(basename "$migration_file")"
            PGPASSWORD="$POSTGRES_PASSWORD" psql -h "$POSTGRES_HOST" -p "$POSTGRES_PORT" -U "$POSTGRES_USER" -d "$POSTGRES_DB" -f "$migration_file"
            if [ $? -eq 0 ]; then
                echo "    ✅ Migration completed successfully"
            else
                echo "    ❌ Migration failed"
                exit 1
            fi
        fi
    done
else
    echo "⚠️  Warning: Migrations directory '$MIGRATIONS_DIR' not found"
fi

echo "✅ Database initialization complete!"
