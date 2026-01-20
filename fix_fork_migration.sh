#!/bin/bash
# Script de limpieza para problemas de compilación después de fork/migración
# Uso: chmod +x fix_fork_migration.sh && ./fix_fork_migration.sh

set -e

echo "========================================"
echo "Limpieza de Artefactos Post-Migracion"
echo "========================================"
echo

# Paso 1: Cerrar procesos de Rust (si hay)
echo "[1/6] Cerrando procesos de Rust..."
pkill -f cargo 2>/dev/null || true
pkill -f rustc 2>/dev/null || true
sleep 2

# Paso 2: Eliminar target/
echo "[2/6] Eliminando directorio target/..."
if [ -d "target" ]; then
    rm -rf target
    echo "  ✓ target/ eliminado"
else
    echo "  ℹ target/ no existe"
fi

# Paso 3: Eliminar Cargo.lock
echo "[3/6] Eliminando Cargo.lock..."
if [ -f "Cargo.lock" ]; then
    rm -f Cargo.lock
    echo "  ✓ Cargo.lock eliminado"
else
    echo "  ℹ Cargo.lock no existe"
fi

# Paso 4: Eliminar cachés de sqlx
echo "[4/6] Eliminando cachés de sqlx..."
if [ -d ".sqlx" ]; then
    rm -rf .sqlx
    echo "  ✓ .sqlx/ eliminado"
else
    echo "  ℹ .sqlx/ no existe"
fi

# Paso 5: Limpiar cachés de cargo (opcional)
echo "[5/6] Limpiando cachés de cargo (skip - opcional)..."
# rm -rf ~/.cargo/registry/cache/* 2>/dev/null || true
# rm -rf ~/.cargo/.package-cache 2>/dev/null || true
echo "  ℹ Cachés de cargo preservados (descomentar si es necesario)"

# Paso 6: Esperar
echo "[6/6] Esperando liberación de archivos..."
sleep 2

echo
echo "========================================"
echo "Limpieza completada!"
echo "========================================"
echo
echo "Próximos pasos:"
echo "  1. Regenerar Cargo.lock: cargo generate-lockfile"
echo "  2. Compilar: cargo build --features redis,observability"
echo
