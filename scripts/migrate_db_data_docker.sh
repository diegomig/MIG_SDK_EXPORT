#!/bin/bash
# Script de migración usando pg_dump y psql directamente desde Docker
# Más robusto que usar psycopg2 en Windows

set -e

echo "🚀 Iniciando migración de datos usando pg_dump/psql..."

SOURCE_CONTAINER="arbitrage-postgres"
TARGET_CONTAINER="mig-topology-postgres"
SOURCE_DB="arbitrage_bot"
TARGET_DB="mig_topology"
SOURCE_SCHEMA="arbitrage"
TARGET_SCHEMA="mig_topology"
SOURCE_USER="arbitrage_user"
TARGET_USER="mig_topology_user"

# Verificar contenedores
if ! docker ps | grep -q "$SOURCE_CONTAINER"; then
    echo "❌ Error: $SOURCE_CONTAINER no está corriendo"
    exit 1
fi

if ! docker ps | grep -q "$TARGET_CONTAINER"; then
    echo "❌ Error: $TARGET_CONTAINER no está corriendo"
    exit 1
fi

echo "✅ Contenedores verificados"

# Tablas a migrar
TABLES=(
    "tokens"
    "pools"
    "dex_state"
    "pool_state_snapshots"
    "token_relations"
    "audit_log"
    "graph_weights"
    "pool_statistics"
    "dex_statistics"
    "configurations"
    "event_index"
)

echo ""
echo "📦 Migrando tablas..."

for table in "${TABLES[@]}"; do
    echo ""
    echo "Migrando: $table"
    
    # Verificar que la tabla existe en origen
    if ! docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -t -c "SELECT 1 FROM information_schema.tables WHERE table_schema='$SOURCE_SCHEMA' AND table_name='$table';" | grep -q 1; then
        echo "  ⏭️  Tabla no existe en origen, omitiendo"
        continue
    fi
    
    # Verificar que la tabla existe en destino
    if ! docker exec $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -t -c "SELECT 1 FROM information_schema.tables WHERE table_schema='$TARGET_SCHEMA' AND table_name='$table';" | grep -q 1; then
        echo "  ⏭️  Tabla no existe en destino, omitiendo"
        continue
    fi
    
    # Obtener columnas comunes
    # Para pool_statistics, excluir columnas de profit
    if [ "$table" = "pool_statistics" ]; then
        COLUMNS="pool_address,tvl_usd,volatility_bps,volatility_sample_count,updated_at"
    else
        # Obtener todas las columnas de la tabla origen
        COLUMNS=$(docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -t -c "
            SELECT string_agg(column_name, ',' ORDER BY ordinal_position)
            FROM information_schema.columns
            WHERE table_schema='$SOURCE_SCHEMA' AND table_name='$table';
        " | tr -d ' \n')
    fi
    
    if [ -z "$COLUMNS" ]; then
        echo "  ⏭️  No se encontraron columnas, omitiendo"
        continue
    fi
    
    # Exportar datos
    echo "  📤 Exportando datos..."
    docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -c "
        COPY (SELECT $COLUMNS FROM $SOURCE_SCHEMA.$table) TO STDOUT WITH CSV HEADER
    " > /tmp/migrate_${table}.csv
    
    # Contar filas
    ROW_COUNT=$(wc -l < /tmp/migrate_${table}.csv | tr -d ' ')
    ROW_COUNT=$((ROW_COUNT - 1))  # Restar header
    
    if [ "$ROW_COUNT" -eq 0 ]; then
        echo "  ⏭️  Tabla vacía, omitiendo"
        rm -f /tmp/migrate_${table}.csv
        continue
    fi
    
    echo "  📥 Importando $ROW_COUNT filas..."
    
    # Importar datos usando COPY
    docker exec -i $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB <<EOF
        BEGIN;
        -- Usar ON CONFLICT para evitar duplicados
        CREATE TEMP TABLE temp_${table} (LIKE ${TARGET_SCHEMA}.${table});
        COPY temp_${table} FROM STDIN WITH CSV HEADER;
        $(cat /tmp/migrate_${table}.csv)
        \.
        INSERT INTO ${TARGET_SCHEMA}.${table} 
        SELECT * FROM temp_${table}
        ON CONFLICT DO NOTHING;
        COMMIT;
EOF
    
    if [ $? -eq 0 ]; then
        echo "  ✅ Migración completa: $ROW_COUNT filas"
    else
        echo "  ❌ Error en la migración"
    fi
    
    rm -f /tmp/migrate_${table}.csv
done

echo ""
echo "✅ Migración completada"
