# Guía de Migración de Base de Datos

## 📋 Resumen

Este documento describe cómo migrar datos desde la base de datos original (`arbitrage-bot-v2`) a la base de datos del fork (`MIG_SDK_EXPORT`).

## 🎯 Objetivo

Migrar datos históricos de topología (tokens, pools, estados, etc.) desde el proyecto original al fork, excluyendo datos específicos de trading/arbitrage.

## 📊 Tablas que se Migran

Las siguientes tablas se migran completamente:

- ✅ `tokens` - Información de tokens
- ✅ `pools` - Información de pools de liquidez
- ✅ `dex_state` - Estado de procesamiento por DEX
- ✅ `pool_state_snapshots` - Snapshots históricos de pools
- ✅ `token_relations` - Relaciones entre tokens (wrap/bridge)
- ✅ `audit_log` - Logs de auditoría
- ✅ `graph_weights` - Pesos del grafo de pools
- ✅ `pool_statistics` - Estadísticas de pools (solo columnas compatibles)
- ✅ `dex_statistics` - Estadísticas por DEX
- ✅ `configurations` - Configuraciones del sistema
- ✅ `event_index` - Índice de eventos procesados

## 🚫 Tablas que NO se Migran

Las siguientes tablas son específicas de trading/arbitrage y **NO** se migran:

- ❌ `route_catalog` - Catálogo de rutas de arbitrage
- ❌ `route_catalog_history` - Historial de rutas
- ❌ `opportunities` - Oportunidades de trading detectadas
- ❌ `opportunity_diagnostics` - Diagnósticos de oportunidades
- ❌ `executions` - Ejecuciones de trades

## ⚠️ Diferencias de Esquema

### Schema Name
- **Origen**: `arbitrage`
- **Destino**: `mig_topology`

### Tabla `pool_statistics`

**Columnas en origen (arbitrage-bot-v2)**:
- `pool_address`
- `tvl_usd`
- `avg_profit_usd` ❌ (no existe en destino)
- `profit_sample_count` ❌ (no existe en destino)
- `last_profit_usd` ❌ (no existe en destino)
- `volatility_bps`
- `volatility_sample_count`
- `updated_at`

**Columnas en destino (MIG_SDK_EXPORT)**:
- `pool_address`
- `tvl_usd`
- `volatility_bps`
- `volatility_sample_count`
- `updated_at`

El script de migración automáticamente excluye las columnas relacionadas con profit que no existen en el destino.

## 🔧 Requisitos Previos

1. **Contenedores Docker corriendo**:
   ```bash
   # Base de datos origen
   cd arbitrage-bot-v2
   docker compose up -d postgres
   
   # Base de datos destino
   cd MIG_SDK_EXPORT/docker_infrastructure
   docker compose up -d postgres
   ```

2. **Python 3.7+** instalado

3. **Dependencias Python**:
   ```bash
   pip install psycopg2-binary
   ```

## 🚀 Ejecución de la Migración

### Opción 1: Script Python Directo

```bash
cd MIG_SDK_EXPORT/scripts
python3 migrate_db_data.py
```

### Opción 2: Script Bash (Linux/Mac/WSL)

```bash
cd MIG_SDK_EXPORT/scripts
chmod +x migrate_db_data.sh
./migrate_db_data.sh
```

### Opción 3: Script PowerShell (Windows)

```powershell
cd MIG_SDK_EXPORT/scripts
.\migrate_db_data.ps1
```

## 📝 Proceso de Migración

1. **Verificación de Conexiones**: El script verifica que ambas bases de datos estén accesibles.

2. **Verificación de Tablas**: Se verifica qué tablas existen en ambas bases de datos.

3. **Confirmación**: Se solicita confirmación antes de comenzar la migración.

4. **Migración por Tabla**: Cada tabla se migra de forma independiente:
   - Se identifican columnas comunes
   - Se migran datos en lotes de 1000 filas
   - Se usa `ON CONFLICT` para evitar duplicados

5. **Resumen**: Al finalizar, se muestra un resumen de la migración.

6. **Log**: Se genera un archivo JSON con los detalles de la migración.

## 🔍 Verificación Post-Migración

Después de la migración, verifica los datos:

```sql
-- Conectar a la base de datos destino
docker exec -it mig-topology-postgres psql -U mig_topology_user -d mig_topology

-- Verificar conteo de filas
SELECT 
    'tokens' as tabla, COUNT(*) as filas FROM mig_topology.tokens
UNION ALL
SELECT 'pools', COUNT(*) FROM mig_topology.pools
UNION ALL
SELECT 'dex_state', COUNT(*) FROM mig_topology.dex_state
UNION ALL
SELECT 'pool_state_snapshots', COUNT(*) FROM mig_topology.pool_state_snapshots
UNION ALL
SELECT 'graph_weights', COUNT(*) FROM mig_topology.graph_weights
UNION ALL
SELECT 'pool_statistics', COUNT(*) FROM mig_topology.pool_statistics;
```

## ⚠️ Advertencias

1. **Datos Existentes**: Si la tabla destino ya tiene datos, el script preguntará antes de continuar. Los datos se actualizarán usando `ON CONFLICT DO UPDATE`.

2. **Backup**: Se recomienda hacer un backup de la base de datos destino antes de migrar:
   ```bash
   docker exec mig-topology-postgres pg_dump -U mig_topology_user mig_topology > backup_before_migration.sql
   ```

3. **Rendimiento**: La migración puede tardar varios minutos dependiendo del volumen de datos.

4. **Conectividad**: Asegúrate de que ambos contenedores estén en la misma red Docker o que los puertos estén expuestos correctamente.

## 🐛 Solución de Problemas

### Error: "No se pudieron establecer las conexiones"

- Verifica que los contenedores estén corriendo: `docker ps`
- Verifica las credenciales en el script
- Verifica que los puertos estén expuestos: `docker compose ps`

### Error: "Tabla no existe"

- Verifica que el schema existe en ambas bases de datos
- Ejecuta la inicialización de la base de datos destino primero:
  ```bash
  cd MIG_SDK_EXPORT
  # Ejecutar el binario que inicializa la DB
  ```

### Error: "Columnas incompatibles"

- El script maneja automáticamente las diferencias de columnas
- Si hay un error específico, revisa el log JSON generado

## 📚 Referencias

- [MIGRATION_IMPACT_ANALYSIS.md](../MIGRATION_IMPACT_ANALYSIS.md) - Análisis de impacto de migración
- [MIGRATION_PROGRESS.md](../MIGRATION_PROGRESS.md) - Progreso de migración de código
- [SCHEMA_MIGRATION_PLAN.md](../SCHEMA_MIGRATION_PLAN.md) - Plan de migración de schema
