@echo off
REM Script de limpieza para problemas de compilación después de fork/migración
REM Uso: fix_fork_migration.bat

echo ========================================
echo Limpieza de Artefactos Post-Migracion
echo ========================================
echo.

REM Paso 1: Cerrar procesos de Rust
echo [1/6] Cerrando procesos de Rust...
taskkill /F /IM cargo.exe >nul 2>&1
taskkill /F /IM rustc.exe >nul 2>&1
timeout /t 2 /nobreak >nul

REM Paso 2: Eliminar target/
echo [2/6] Eliminando directorio target/...
if exist target (
    rmdir /S /Q target
    echo   ✓ target/ eliminado
) else (
    echo   ℹ target/ no existe
)

REM Paso 3: Eliminar Cargo.lock
echo [3/6] Eliminando Cargo.lock...
if exist Cargo.lock (
    del /F /Q Cargo.lock
    echo   ✓ Cargo.lock eliminado
) else (
    echo   ℹ Cargo.lock no existe
)

REM Paso 4: Eliminar cachés de sqlx
echo [4/6] Eliminando cachés de sqlx...
if exist .sqlx (
    rmdir /S /Q .sqlx
    echo   ✓ .sqlx/ eliminado
) else (
    echo   ℹ .sqlx/ no existe
)

REM Paso 5: Limpiar cachés de cargo (opcional, comentado por defecto)
echo [5/6] Limpiando cachés de cargo (skip - opcional)...
REM rmdir /S /Q "%USERPROFILE%\.cargo\registry\cache" 2>nul
REM rmdir /S /Q "%USERPROFILE%\.cargo\.package-cache" 2>nul
echo   ℹ Cachés de cargo preservados (descomentar si es necesario)

REM Paso 6: Esperar liberación de archivos
echo [6/6] Esperando liberación de archivos...
timeout /t 3 /nobreak >nul

echo.
echo ========================================
echo Limpieza completada!
echo ========================================
echo.
echo Próximos pasos:
echo   1. Regenerar Cargo.lock: cargo generate-lockfile
echo   2. Compilar: cargo build --features redis,observability
echo   3. Si falla, usar WSL (recomendado)
echo.
