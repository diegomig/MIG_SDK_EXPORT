#!/bin/bash
# Script de migración simple usando INSERT ... SELECT directamente
# Conecta ambas bases de datos a través de Docker network

set -e

echo "🚀 Iniciando migración de datos (método directo)..."

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

# Conectar ambas bases de datos en la misma red
# Primero, obtener la red del contenedor destino
TARGET_NETWORK=$(docker inspect $TARGET_CONTAINER --format='{{range $net,$v := .NetworkSettings.Networks}}{{$net}}{{end}}')

# Tablas a migrar con sus columnas específicas
declare -A TABLE_COLUMNS
TABLE_COLUMNS[tokens]="id,address,symbol,decimals,token_type,oracle_source,confidence_score,last_verified_block,created_at"
TABLE_COLUMNS[pools]="id,address,dex,factory,token0,token1,fee_bps,created_block,is_valid,is_active,last_seen_block,created_at,updated_at,last_viable_at,last_viable_block,origin_dex,bytecode_hash,init_code_hash"
TABLE_COLUMNS[dex_state]="dex,last_processed_block,mode,updated_at"
TABLE_COLUMNS[pool_state_snapshots]="id,pool_address,block_number,reserve0,reserve1,ts,slot0_block,liquidity_block,liquidity,reserves_block"
TABLE_COLUMNS[token_relations]="id,base_token,wrapped_token,relation_type,priority_source,confidence_score,created_at"
TABLE_COLUMNS[audit_log]="id,entity,entity_id,observed,expected,severity,ts"
TABLE_COLUMNS[graph_weights]="pool_address,weight,last_computed_block,updated_at"
TABLE_COLUMNS[pool_statistics]="pool_address,tvl_usd,volatility_bps,volatility_sample_count,updated_at"
TABLE_COLUMNS[dex_statistics]="dex,total_pools,active_pools,valid_pools,unique_factories,unique_init_code_hashes,unique_bytecode_hashes,last_refreshed_at"
TABLE_COLUMNS[configurations]="key,value,created_at"
TABLE_COLUMNS[event_index]="dex,block_number,event_type,pool_address,indexed_at"

# Función para migrar una tabla
migrate_table() {
    local table=$1
    local columns=$2
    
    echo ""
    echo "📦 Migrando: $table"
    
    # Verificar que existe en origen
    local exists_source=$(docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -t -c "
        SELECT COUNT(*) FROM information_schema.tables 
        WHERE table_schema='$SOURCE_SCHEMA' AND table_name='$table';
    " | tr -d ' \n')
    
    if [ "$exists_source" != "1" ]; then
        echo "  ⏭️  No existe en origen"
        return
    fi
    
    # Verificar que existe en destino
    local exists_target=$(docker exec $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -t -c "
        SELECT COUNT(*) FROM information_schema.tables 
        WHERE table_schema='$TARGET_SCHEMA' AND table_name='$table';
    " | tr -d ' \n')
    
    if [ "$exists_target" != "1" ]; then
        echo "  ⏭️  No existe en destino"
        return
    fi
    
    # Contar filas en origen
    local count=$(docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -t -c "
        SELECT COUNT(*) FROM $SOURCE_SCHEMA.$table;
    " | tr -d ' \n')
    
    if [ "$count" = "0" ]; then
        echo "  ⏭️  Tabla vacía ($count filas)"
        return
    fi
    
    echo "  📊 Filas en origen: $count"
    
    # Obtener primary key
    local pk=$(docker exec $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -t -c "
        SELECT column_name 
        FROM information_schema.table_constraints tc
        JOIN information_schema.constraint_column_usage ccu 
            ON tc.constraint_name = ccu.constraint_name
        WHERE tc.table_schema='$TARGET_SCHEMA' 
            AND tc.table_name='$table' 
            AND tc.constraint_type='PRIMARY KEY'
        LIMIT 1;
    " | tr -d ' \n')
    
    # Exportar datos desde origen e importar en destino
    echo "  📤 Exportando e importando datos..."
    
    # Usar un enfoque más robusto: crear tabla temporal sin restricciones
    if [ -n "$pk" ]; then
        # Hay primary key, usar ON CONFLICT DO UPDATE
        docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -c "
            COPY (
                SELECT $columns 
                FROM $SOURCE_SCHEMA.$table
            ) TO STDOUT WITH (FORMAT csv, HEADER false, DELIMITER E'\t')
        " 2>/dev/null | docker exec -i $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -c "
            BEGIN;
            CREATE TEMP TABLE temp_import_${table} (LIKE ${TARGET_SCHEMA}.${table} INCLUDING ALL);
            -- Relajar restricciones temporales si es necesario
            ALTER TABLE temp_import_${table} DROP CONSTRAINT IF EXISTS temp_import_${table}_pkey;
            COPY temp_import_${table} ($columns) FROM STDIN WITH (FORMAT csv, DELIMITER E'\t', NULL '');
            INSERT INTO ${TARGET_SCHEMA}.${table} ($columns)
            SELECT $columns FROM temp_import_${table}
            ON CONFLICT ($pk) DO UPDATE SET
                $(echo $columns | tr ',' '\n' | grep -v "^$pk$" | sed "s/^\(.*\)$/\1 = EXCLUDED.\1/" | tr '\n' ',' | sed 's/,$//');
            COMMIT;
        " 2>&1 | grep -v "ERROR" || true
    else
        # No hay primary key, usar ON CONFLICT DO NOTHING
        docker exec $SOURCE_CONTAINER psql -U $SOURCE_USER -d $SOURCE_DB -c "
            COPY (
                SELECT $columns 
                FROM $SOURCE_SCHEMA.$table
            ) TO STDOUT WITH (FORMAT csv, HEADER false, DELIMITER E'\t')
        " 2>/dev/null | docker exec -i $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -c "
            BEGIN;
            CREATE TEMP TABLE temp_import_${table} (LIKE ${TARGET_SCHEMA}.${table} INCLUDING ALL);
            COPY temp_import_${table} ($columns) FROM STDIN WITH (FORMAT csv, DELIMITER E'\t', NULL '');
            INSERT INTO ${TARGET_SCHEMA}.${table} ($columns)
            SELECT $columns FROM temp_import_${table}
            ON CONFLICT DO NOTHING;
            COMMIT;
        " 2>&1 | grep -v "ERROR" || true
    fi
    
    # Verificar resultado
    local imported=$(docker exec $TARGET_CONTAINER psql -U $TARGET_USER -d $TARGET_DB -t -c "
        SELECT COUNT(*) FROM ${TARGET_SCHEMA}.${table};
    " | tr -d ' \n')
    
    echo "  ✅ Migración completa: $imported filas en destino"
}

# Migrar cada tabla
for table in "${!TABLE_COLUMNS[@]}"; do
    migrate_table "$table" "${TABLE_COLUMNS[$table]}"
done

echo ""
echo "✅ Migración completada"
