# Plan de Migración de Schema: `arbitrage` → `mig_topology`

**Objetivo**: Eliminar completamente todas las referencias a "arbitrage" del código para presentación profesional de grants open source.

**Riesgo**: Alto (afecta ~89 queries SQL + bases de datos existentes)  
**Complejidad**: Media-Alta  
**Duración Estimada**: 4-6 horas (con testing exhaustivo)

---

## 📋 CONTEXTO

Para presentar el SDK como proyecto open source profesional para grants, es **crítico** eliminar todas las referencias a "arbitrage" porque:
- Los evaluadores pueden sospechar intenciones comerciales de trading
- No es profesional tener nombres legacy que sugieran funcionalidad inexistente
- El SDK debe ser claro en su propósito: **topology mapping**, no trading

---

## 🎯 OBJETIVO

Migrar completamente el schema name de `arbitrage` a `mig_topology`:
- ✅ Cambiar todas las queries SQL en código (~89 referencias)
- ✅ Crear script de migración de base de datos
- ✅ Mantener backward compatibility durante transición
- ✅ Testing exhaustivo antes de merge
- ✅ Documentación clara del proceso

---

## 📊 INVENTARIO COMPLETO

### Archivos Afectados

| Archivo | Referencias | Tipo |
|---------|-------------|------|
| `src/database.rs` | ~85 | CREATE SCHEMA, queries SQL |
| `src/event_indexer.rs` | ~3 | queries SQL |
| `src/orchestrator.rs` | ~1 | queries SQL |
| `src/postgres_async_writer.rs` | ~5-10 | queries SQL (verificar) |

**Total estimado**: ~89-95 referencias

---

## 🔄 ESTRATEGIA DE MIGRACIÓN

### Opción Elegida: Migración Completa con Dual Schema (Temporal)

**Fase 1**: Código soporta ambos schemas (backward compatible)  
**Fase 2**: Script de migración DB (crea nuevo schema, migra datos)  
**Fase 3**: Cambiar código para usar solo `mig_topology`  
**Fase 4**: Script de limpieza (eliminar schema antiguo - opcional)

---

## 📝 PLAN DETALLADO PASO A PASO

### FASE 0: PREPARACIÓN Y BACKUP

#### Paso 0.1: Crear Branch de Migración
```bash
git checkout -b schema-migration-arbitrage-to-mig-topology
```

#### Paso 0.2: Inventario Completo de Referencias
```bash
# Buscar TODAS las referencias a "arbitrage" en código SQL
grep -r "arbitrage\." src/ | wc -l
grep -r "CREATE SCHEMA.*arbitrage" src/
grep -r "FROM arbitrage" src/
grep -r "INTO arbitrage" src/
grep -r "UPDATE arbitrage" src/
```

#### Paso 0.3: Documentar Estado Actual
- Listar todas las queries afectadas
- Verificar que no hay referencias hardcoded fuera de queries SQL
- Confirmar que schema name es consistente en todo el código

---

### FASE 1: CAMBIOS EN CÓDIGO (Sin Romper Existente)

#### Paso 1.1: Definir Constante para Schema Name

**Archivo**: `src/database.rs`

```rust
// Schema name for database tables
// NOTE: Migrated from 'arbitrage' to 'mig_topology' for open source clarity
const DB_SCHEMA: &str = "mig_topology";
```

**Beneficio**: Un solo lugar para cambiar el schema name en el futuro.

#### Paso 1.2: Crear Helper Function para Schema Queries

**Archivo**: `src/database.rs`

```rust
/// Get schema-qualified table name
/// Example: schema_table("pools") -> "mig_topology.pools"
fn schema_table(table: &str) -> String {
    format!("{}.{}", DB_SCHEMA, table)
}
```

**Uso**: Reemplazar `"arbitrage.pools"` con `schema_table("pools")` en queries.

#### Paso 1.3: Cambiar CREATE SCHEMA Statement

**Archivo**: `src/database.rs` línea ~148

**Antes**:
```rust
sqlx::query("CREATE SCHEMA IF NOT EXISTS arbitrage")
```

**Después**:
```rust
sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", DB_SCHEMA))
```

#### Paso 1.4: Cambiar Todas las Queries SQL

**Estrategia**: Reemplazar sistemáticamente usando helper function o formato.

**Patrón de Reemplazo**:
- `"arbitrage.pools"` → `&schema_table("pools")` o `&format!("{}.pools", DB_SCHEMA)`
- `"arbitrage.tokens"` → `&schema_table("tokens")`
- `"arbitrage.dex_state"` → `&schema_table("dex_state")`
- etc.

**NOTA**: Para queries complejas con múltiples referencias, usar `format!` macro.

**Ejemplo**:
```rust
// Antes
sqlx::query("SELECT * FROM arbitrage.pools WHERE address = $1")

// Después
sqlx::query(&format!("SELECT * FROM {}.pools WHERE address = $1", DB_SCHEMA))
```

#### Paso 1.5: Cambiar INFORMATION_SCHEMA Queries

**Archivo**: `src/database.rs` línea ~128

**Antes**:
```sql
WHERE table_schema = 'arbitrage'
```

**Después**:
```rust
&format!("WHERE table_schema = '{}'", DB_SCHEMA)
```

---

### FASE 2: SCRIPT DE MIGRACIÓN DE BASE DE DATOS

#### Paso 2.1: Crear Script de Migración SQL

**Archivo**: `migrations/001_rename_schema_arbitrage_to_mig_topology.sql`

```sql
-- Migration: Rename schema from 'arbitrage' to 'mig_topology'
-- Date: 2025-01-XX
-- Purpose: Remove trading/arbitrage references for open source presentation

BEGIN;

-- Step 1: Create new schema if it doesn't exist
CREATE SCHEMA IF NOT EXISTS mig_topology;

-- Step 2: Move all tables from old schema to new schema
-- PostgreSQL doesn't support ALTER SCHEMA RENAME, so we use ALTER TABLE SET SCHEMA

DO $$
DECLARE
    table_name text;
BEGIN
    FOR table_name IN 
        SELECT tablename 
        FROM pg_tables 
        WHERE schemaname = 'arbitrage'
    LOOP
        EXECUTE format('ALTER TABLE arbitrage.%I SET SCHEMA mig_topology', table_name);
        RAISE NOTICE 'Moved table: %', table_name;
    END LOOP;
END $$;

-- Step 3: Move all sequences (if any)
DO $$
DECLARE
    seq_name text;
BEGIN
    FOR seq_name IN 
        SELECT sequence_name 
        FROM information_schema.sequences 
        WHERE sequence_schema = 'arbitrage'
    LOOP
        EXECUTE format('ALTER SEQUENCE arbitrage.%I SET SCHEMA mig_topology', seq_name);
        RAISE NOTICE 'Moved sequence: %', seq_name;
    END LOOP;
END $$;

-- Step 4: Move all functions (if any)
DO $$
DECLARE
    func_name text;
    func_args text;
BEGIN
    FOR func_name, func_args IN 
        SELECT routine_name, routine_definition
        FROM information_schema.routines 
        WHERE routine_schema = 'arbitrage'
    LOOP
        -- Functions are complex to move - may need manual migration
        RAISE NOTICE 'Function found (may need manual migration): %', func_name;
    END LOOP;
END $$;

-- Step 5: Verify migration
DO $$
DECLARE
    table_count integer;
BEGIN
    SELECT COUNT(*) INTO table_count
    FROM information_schema.tables
    WHERE table_schema = 'mig_topology';
    
    IF table_count = 0 THEN
        RAISE EXCEPTION 'Migration failed: No tables found in mig_topology schema';
    END IF;
    
    RAISE NOTICE 'Migration successful: % tables moved to mig_topology schema', table_count;
END $$;

-- Step 6: Drop old schema (OPTIONAL - comment out if you want to keep for backup)
-- WARNING: This is irreversible. Only run after verifying migration is successful.
-- DROP SCHEMA IF EXISTS arbitrage CASCADE;

COMMIT;

-- Verification queries (run separately to verify):
-- SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'mig_topology';
-- SELECT tablename FROM pg_tables WHERE schemaname = 'mig_topology';
```

#### Paso 2.2: Crear Script de Rollback (Por Si Acaso)

**Archivo**: `migrations/001_rollback.sql`

```sql
-- Rollback: Move schema back from mig_topology to arbitrage
-- WARNING: Only use if migration failed and you need to revert

BEGIN;

CREATE SCHEMA IF NOT EXISTS arbitrage;

DO $$
DECLARE
    table_name text;
BEGIN
    FOR table_name IN 
        SELECT tablename 
        FROM pg_tables 
        WHERE schemaname = 'mig_topology'
    LOOP
        EXECUTE format('ALTER TABLE mig_topology.%I SET SCHEMA arbitrage', table_name);
        RAISE NOTICE 'Moved table back: %', table_name;
    END LOOP;
END $$;

COMMIT;
```

---

### FASE 3: TESTING EXHAUSTIVO

#### Paso 3.1: Testing de Código (Sin DB)

```bash
# Verificar que compila
cargo check

# Verificar que no hay referencias hardcoded a "arbitrage"
grep -r "arbitrage\." src/ | grep -v "// NOTE:" | grep -v "legacy"
```

#### Paso 3.2: Testing con Base de Datos Vacía

```bash
# Crear nueva base de datos de test
createdb mig_topology_test

# Ejecutar código (debe crear schema mig_topology)
DATABASE_URL="postgresql://user:pass@localhost/mig_topology_test" cargo test

# Verificar schema creado
psql mig_topology_test -c "\dn"
psql mig_topology_test -c "SELECT schemaname FROM pg_tables WHERE tablename='pools';"
```

#### Paso 3.3: Testing de Migración (Con Datos Existentes)

```bash
# Crear base de datos con schema antiguo
createdb mig_topology_migration_test
psql mig_topology_migration_test -c "CREATE SCHEMA arbitrage;"
# ... crear algunas tablas de prueba ...

# Ejecutar script de migración
psql mig_topology_migration_test -f migrations/001_rename_schema_arbitrage_to_mig_topology.sql

# Verificar que todas las tablas fueron movidas
psql mig_topology_migration_test -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'mig_topology';"

# Ejecutar código con nueva base
DATABASE_URL="postgresql://user:pass@localhost/mig_topology_migration_test" cargo test
```

#### Paso 3.4: Testing de Integración Completa

```bash
# Test completo: crear schema, insertar datos, leer datos
cargo test --test integration_tests

# Verificar que no hay queries fallando
# (revisar logs de tests)
```

---

### FASE 4: DOCUMENTACIÓN Y COMMIT

#### Paso 4.1: Actualizar Documentación

**Archivos a actualizar**:
- `README.md` - Si menciona schema name
- `docs/ARCHITECTURE.md` - Si documenta schema
- Comentarios en código - Actualizar referencias

#### Paso 4.2: Commit Message Profesional

```
refactor(database): migrate schema from 'arbitrage' to 'mig_topology'

BREAKING CHANGE: Database schema renamed for open source clarity

- Renamed schema from 'arbitrage' to 'mig_topology' to remove trading references
- Updated all SQL queries (~89 references across 4 files)
- Added migration script for existing databases
- Schema name is now defined in DB_SCHEMA constant for maintainability

Migration guide:
1. Run migrations/001_rename_schema_arbitrage_to_mig_topology.sql
2. Verify migration with verification queries
3. Update DATABASE_URL if needed

This change removes all references to "arbitrage" to present the SDK
as a clean, professional open source project focused on topology mapping,
not trading or arbitrage.
```

#### Paso 4.3: Crear Migration Guide para Usuarios

**Archivo**: `MIGRATION_GUIDE.md`

```markdown
# Migration Guide: Schema Rename (arbitrage → mig_topology)

## Overview

The database schema has been renamed from `arbitrage` to `mig_topology` to better reflect the SDK's purpose as a topology mapping library, not a trading system.

## For New Installations

No action required. The SDK will create the `mig_topology` schema automatically.

## For Existing Installations

### Step 1: Backup Your Database

```bash
pg_dump your_database > backup_before_migration.sql
```

### Step 2: Run Migration Script

```bash
psql your_database -f migrations/001_rename_schema_arbitrage_to_mig_topology.sql
```

### Step 3: Verify Migration

```sql
-- Should show all your tables
SELECT tablename FROM pg_tables WHERE schemaname = 'mig_topology';

-- Should show 0 tables in old schema
SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'arbitrage';
```

### Step 4: Update Code

Update to the latest SDK version that uses `mig_topology` schema.

### Step 5: (Optional) Remove Old Schema

Only after verifying everything works:

```sql
DROP SCHEMA IF EXISTS arbitrage CASCADE;
```
```

---

## ✅ CHECKLIST DE VALIDACIÓN

### Pre-Merge Checklist

- [ ] Todos los archivos modificados compilan sin errores
- [ ] No hay referencias hardcoded a "arbitrage" (excepto en comentarios de migración)
- [ ] Script de migración probado en base de datos de test
- [ ] Tests de integración pasan con nuevo schema
- [ ] Documentación actualizada
- [ ] Migration guide creado
- [ ] Commit message profesional con BREAKING CHANGE
- [ ] Code review realizado (si aplica)

### Post-Merge Verification

- [ ] Verificar que CI/CD pasa
- [ ] Verificar que no hay queries fallando en producción (si aplica)
- [ ] Monitorear logs por errores de schema

---

## 🚨 RIESGOS Y MITIGACIONES

### Riesgo Alto: Datos Existentes en Producción

**Mitigación**:
- Script de migración probado exhaustivamente
- Backup obligatorio antes de migración
- Script de rollback disponible
- Testing en staging primero

### Riesgo Medio: Queries Completas con Múltiples Referencias

**Mitigación**:
- Revisar cada query individualmente
- Testing exhaustivo de cada query
- Usar helper function para consistencia

### Riesgo Bajo: Referencias Hardcoded Faltantes

**Mitigación**:
- Búsqueda exhaustiva con grep
- Code review
- Testing de integración

---

## 📊 ESTIMACIÓN DE TIEMPO

| Fase | Tareas | Tiempo Estimado |
|------|--------|-----------------|
| Fase 0 | Preparación e inventario | 30 min |
| Fase 1 | Cambios en código | 2-3 horas |
| Fase 2 | Script de migración | 1 hora |
| Fase 3 | Testing exhaustivo | 1-2 horas |
| Fase 4 | Documentación | 30 min |
| **Total** | | **4-6 horas** |

---

## 🎯 RESULTADO FINAL

Después de esta migración:

✅ **Cero referencias a "arbitrage"** en código SQL  
✅ **Schema name profesional**: `mig_topology`  
✅ **Migración segura** para bases existentes  
✅ **Código open source limpio** listo para grants  
✅ **Documentación completa** del proceso  

El SDK se presenta como un proyecto profesional de **topology mapping**, sin rastros de trading o arbitrage.
