#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Script de migración de datos desde arbitrage-bot-v2 a MIG_SDK_EXPORT

Migra datos de la base de datos original (arbitrage_bot, schema arbitrage) 
a la base de datos del fork (mig_topology, schema mig_topology).

Tablas que se migran:
- tokens
- pools
- dex_state
- pool_state_snapshots
- token_relations
- audit_log
- graph_weights
- pool_statistics (solo columnas compatibles)
- dex_statistics
- configurations
- event_index

Tablas que NO se migran (específicas de trading):
- route_catalog
- route_catalog_history
- opportunities
- opportunity_diagnostics
- executions
"""

import sys
import io
import os

# Configurar stdout para UTF-8 en Windows (debe estar antes de cualquier print)
if sys.platform == 'win32':
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
    sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

import psycopg2
from psycopg2.extras import RealDictCursor
from typing import List, Dict, Any
import json
from datetime import datetime

# Configuración de la base de datos original
# Nota: Si el puerto 5432 está ocupado, usar 5433
SOURCE_DB_CONFIG = {
    'host': 'localhost',
    'port': int(os.environ.get('SOURCE_DB_PORT', '5433')),  # Default 5433 si 5432 está ocupado
    'database': 'arbitrage_bot',
    'user': 'arbitrage_user',
    'password': 'arbitrage_pass'
}

# Configuración de la base de datos destino
TARGET_DB_CONFIG = {
    'host': 'localhost',
    'port': 5432,
    'database': 'mig_topology',
    'user': 'mig_topology_user',
    'password': 'mig_topology_pass'
}

SOURCE_SCHEMA = 'arbitrage'
TARGET_SCHEMA = 'mig_topology'

# Tablas a migrar (solo las que existen en ambas bases de datos)
TABLES_TO_MIGRATE = [
    'tokens',
    'pools',
    'dex_state',
    'pool_state_snapshots',
    'token_relations',
    'audit_log',
    'graph_weights',
    'pool_statistics',
    'dex_statistics',
    'configurations',
    'event_index'
]

# Mapeo de columnas para tablas con diferencias
COLUMN_MAPPING = {
    'pool_statistics': {
        # Columnas en origen que no existen en destino
        'exclude': ['avg_profit_usd', 'profit_sample_count', 'last_profit_usd'],
        # Columnas comunes
        'include': ['pool_address', 'tvl_usd', 'volatility_bps', 'volatility_sample_count', 'updated_at']
    }
}


def get_connection(config: Dict[str, Any], schema: str = None):
    """Crea una conexión a la base de datos"""
    # Agregar client_encoding explícitamente para evitar problemas de codificación
    config_with_encoding = config.copy()
    config_with_encoding['client_encoding'] = 'UTF8'
    conn = psycopg2.connect(**config_with_encoding)
    if schema:
        with conn.cursor() as cur:
            cur.execute(f"SET search_path TO {schema}, public")
    return conn


def get_table_columns(conn, schema: str, table: str) -> List[str]:
    """Obtiene la lista de columnas de una tabla"""
    query = """
        SELECT column_name 
        FROM information_schema.columns 
        WHERE table_schema = %s AND table_name = %s
        ORDER BY ordinal_position
    """
    with conn.cursor() as cur:
        cur.execute(query, (schema, table))
        return [row[0] for row in cur.fetchall()]


def get_table_row_count(conn, schema: str, table: str) -> int:
    """Obtiene el número de filas en una tabla"""
    query = f'SELECT COUNT(*) FROM "{schema}"."{table}"'
    with conn.cursor() as cur:
        cur.execute(query)
        return cur.fetchone()[0]


def migrate_table(
    source_conn, 
    target_conn, 
    table: str,
    source_schema: str,
    target_schema: str
) -> Dict[str, Any]:
    """Migra una tabla completa de origen a destino"""
    print(f"\n📦 Migrando tabla: {table}")
    
    # Obtener columnas de origen y destino
    source_columns = get_table_columns(source_conn, source_schema, table)
    target_columns = get_table_columns(target_conn, target_schema, table)
    
    # Aplicar mapeo de columnas si existe
    if table in COLUMN_MAPPING:
        mapping = COLUMN_MAPPING[table]
        if 'exclude' in mapping:
            source_columns = [c for c in source_columns if c not in mapping['exclude']]
        if 'include' in mapping:
            source_columns = [c for c in source_columns if c in mapping['include']]
    
    # Encontrar columnas comunes
    common_columns = [c for c in source_columns if c in target_columns]
    
    if not common_columns:
        return {
            'table': table,
            'status': 'skipped',
            'reason': 'No hay columnas comunes',
            'rows_migrated': 0
        }
    
    print(f"   Columnas comunes: {len(common_columns)} de {len(source_columns)} (origen) / {len(target_columns)} (destino)")
    
    # Verificar si hay datos
    source_count = get_table_row_count(source_conn, source_schema, table)
    target_count = get_table_row_count(target_conn, target_schema, table)
    
    print(f"   Filas en origen: {source_count}")
    print(f"   Filas en destino: {target_count}")
    
    if source_count == 0:
        return {
            'table': table,
            'status': 'skipped',
            'reason': 'Tabla vacía en origen',
            'rows_migrated': 0
        }
    
    # Si ya hay datos en destino, preguntar
    if target_count > 0:
        response = input(f"   ⚠️  La tabla {table} ya tiene {target_count} filas. ¿Continuar? (s/n): ")
        if response.lower() != 's':
            return {
                'table': table,
                'status': 'skipped',
                'reason': 'Usuario canceló',
                'rows_migrated': 0
            }
    
    # Construir query de inserción
    columns_str = ', '.join(f'"{c}"' for c in common_columns)
    placeholders = ', '.join(['%s'] * len(common_columns))
    
    # Usar ON CONFLICT para evitar duplicados
    # Necesitamos identificar la primary key o unique constraint
    pk_query = f"""
        SELECT column_name 
        FROM information_schema.table_constraints tc
        JOIN information_schema.constraint_column_usage ccu 
            ON tc.constraint_name = ccu.constraint_name
        WHERE tc.table_schema = %s 
            AND tc.table_name = %s 
            AND tc.constraint_type = 'PRIMARY KEY'
    """
    
    with target_conn.cursor() as cur:
        cur.execute(pk_query, (target_schema, table))
        pk_result = cur.fetchone()
        pk_column = pk_result[0] if pk_result else None
    
    if pk_column and pk_column in common_columns:
        # Usar ON CONFLICT DO UPDATE
        update_clause = ', '.join(f'"{c}" = EXCLUDED."{c}"' for c in common_columns if c != pk_column)
        insert_query = f"""
            INSERT INTO "{target_schema}"."{table}" ({columns_str})
            VALUES ({placeholders})
            ON CONFLICT ("{pk_column}") DO UPDATE SET {update_clause}
        """
    else:
        # Insertar sin conflicto (puede fallar si hay duplicados)
        insert_query = f"""
            INSERT INTO "{target_schema}"."{table}" ({columns_str})
            VALUES ({placeholders})
            ON CONFLICT DO NOTHING
        """
    
    # Leer datos de origen
    select_query = f'SELECT {columns_str} FROM "{source_schema}"."{table}"'
    
    rows_migrated = 0
    batch_size = 1000
    
    try:
        with source_conn.cursor(cursor_factory=RealDictCursor) as source_cur:
            source_cur.execute(select_query)
            
            batch = []
            for row in source_cur:
                values = [row[col] for col in common_columns]
                batch.append(tuple(values))
                
                if len(batch) >= batch_size:
                    with target_conn.cursor() as target_cur:
                        target_cur.executemany(insert_query, batch)
                        target_conn.commit()
                    rows_migrated += len(batch)
                    print(f"   ✅ Migradas {rows_migrated} filas...", end='\r')
                    batch = []
            
            # Migrar el último batch
            if batch:
                with target_conn.cursor() as target_cur:
                    target_cur.executemany(insert_query, batch)
                    target_conn.commit()
                rows_migrated += len(batch)
        
        print(f"   ✅ Migración completa: {rows_migrated} filas migradas")
        
        return {
            'table': table,
            'status': 'success',
            'rows_migrated': rows_migrated,
            'columns_migrated': len(common_columns)
        }
    
    except Exception as e:
        target_conn.rollback()
        print(f"   ❌ Error: {e}")
        return {
            'table': table,
            'status': 'error',
            'error': str(e),
            'rows_migrated': rows_migrated
        }


def verify_connections():
    """Verifica que ambas conexiones funcionen"""
    print("🔍 Verificando conexiones...")
    
    try:
        source_conn = get_connection(SOURCE_DB_CONFIG)
        source_conn.close()
        print("   ✅ Conexión a base de datos origen: OK")
    except Exception as e:
        print(f"   ❌ Error conectando a base de datos origen: {e}")
        return False
    
    try:
        target_conn = get_connection(TARGET_DB_CONFIG)
        target_conn.close()
        print("   ✅ Conexión a base de datos destino: OK")
    except Exception as e:
        print(f"   ❌ Error conectando a base de datos destino: {e}")
        return False
    
    return True


def check_tables_exist(conn, schema: str, tables: List[str]) -> Dict[str, bool]:
    """Verifica qué tablas existen en el schema"""
    query = """
        SELECT table_name 
        FROM information_schema.tables 
        WHERE table_schema = %s AND table_name = ANY(%s)
    """
    with conn.cursor() as cur:
        cur.execute(query, (schema, tables))
        existing = {row[0] for row in cur.fetchall()}
    
    return {table: table in existing for table in tables}


def main():
    """Función principal de migración"""
    print("=" * 70)
    print("🚀 MIGRACIÓN DE DATOS: arbitrage-bot-v2 → MIG_SDK_EXPORT")
    print("=" * 70)
    print(f"Fecha: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print()
    
    # Verificar conexiones
    if not verify_connections():
        print("\n❌ No se pudieron establecer las conexiones. Verifica que:")
        print("   1. Los contenedores Docker estén corriendo")
        print("   2. Las credenciales en el script sean correctas")
        print("   3. Los puertos estén accesibles")
        sys.exit(1)
    
    # Conectar a ambas bases de datos
    print("\n📡 Conectando a las bases de datos...")
    source_conn = get_connection(SOURCE_DB_CONFIG)
    target_conn = get_connection(TARGET_DB_CONFIG)
    
    try:
        # Verificar qué tablas existen
        print("\n📋 Verificando tablas...")
        source_tables = check_tables_exist(source_conn, SOURCE_SCHEMA, TABLES_TO_MIGRATE)
        target_tables = check_tables_exist(target_conn, TARGET_SCHEMA, TABLES_TO_MIGRATE)
        
        print("\nTablas en origen:")
        for table, exists in source_tables.items():
            status = "✅" if exists else "❌"
            print(f"   {status} {table}")
        
        print("\nTablas en destino:")
        for table, exists in target_tables.items():
            status = "✅" if exists else "❌"
            print(f"   {status} {table}")
        
        # Filtrar tablas que existen en ambas
        tables_to_migrate = [
            t for t in TABLES_TO_MIGRATE 
            if source_tables.get(t, False) and target_tables.get(t, False)
        ]
        
        if not tables_to_migrate:
            print("\n❌ No hay tablas comunes para migrar")
            return
        
        print(f"\n📦 Tablas a migrar: {len(tables_to_migrate)}")
        for table in tables_to_migrate:
            print(f"   - {table}")
        
        # Confirmar antes de migrar
        print("\n⚠️  ADVERTENCIA: Esta operación modificará la base de datos destino")
        response = input("¿Deseas continuar? (s/n): ")
        if response.lower() != 's':
            print("❌ Migración cancelada por el usuario")
            return
        
        # Migrar cada tabla
        results = []
        for table in tables_to_migrate:
            result = migrate_table(
                source_conn,
                target_conn,
                table,
                SOURCE_SCHEMA,
                TARGET_SCHEMA
            )
            results.append(result)
        
        # Resumen
        print("\n" + "=" * 70)
        print("📊 RESUMEN DE MIGRACIÓN")
        print("=" * 70)
        
        total_rows = 0
        success_count = 0
        error_count = 0
        skipped_count = 0
        
        for result in results:
            status = result['status']
            if status == 'success':
                success_count += 1
                total_rows += result.get('rows_migrated', 0)
                print(f"✅ {result['table']}: {result['rows_migrated']} filas migradas")
            elif status == 'error':
                error_count += 1
                print(f"❌ {result['table']}: Error - {result.get('error', 'Unknown')}")
            else:
                skipped_count += 1
                print(f"⏭️  {result['table']}: {result.get('reason', 'Skipped')}")
        
        print(f"\nTotal: {success_count} exitosas, {error_count} errores, {skipped_count} omitidas")
        print(f"Filas migradas: {total_rows}")
        
        # Guardar log
        log_file = f"migration_log_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        with open(log_file, 'w') as f:
            json.dump({
                'timestamp': datetime.now().isoformat(),
                'source_schema': SOURCE_SCHEMA,
                'target_schema': TARGET_SCHEMA,
                'results': results,
                'total_rows_migrated': total_rows
            }, f, indent=2)
        
        print(f"\n📝 Log guardado en: {log_file}")
        
    finally:
        source_conn.close()
        target_conn.close()
        print("\n✅ Conexiones cerradas")


if __name__ == '__main__':
    main()
