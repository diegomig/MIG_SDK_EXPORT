# Troubleshooting Guide

## Problemas Comunes de Compilación en Windows

### Error: `LNK1104: cannot open file 'sqlx_macros-*.dll'` o `LoadLibraryExW failed: Insufficient system resources (error 1450)`

**Síntoma**: El proyecto no compila en Windows con errores del linker relacionados con `sqlx-macros`.

**Causa**: Problema conocido de Windows con el linker de MSVC y procedimiento macros de Rust (específicamente `sqlx-macros`). Esto puede ocurrir por:

1. Archivos bloqueados por procesos anteriores
2. Antivirus (Windows Defender) bloqueando archivos `.pdb` y `.dll`
3. Rutas muy largas (límite de 260 caracteres en Windows)
4. Corrupción en el directorio `target/`
5. **Artefactos de migración/fork**: `Cargo.lock` o `target/` con referencias al proyecto original

**Soluciones (en orden de recomendación):**

#### Solución 1: Limpieza Profunda Post-Migración (RECOMENDADO si el proyecto fue migrado/forkeado)

Si el proyecto fue migrado de otro proyecto (fork, copia, etc.), ejecuta la limpieza completa:

```powershell
# Usar el script automatizado
.\fix_fork_migration.bat

# O manualmente:
taskkill /F /IM cargo.exe 2>$null
taskkill /F /IM rustc.exe 2>$null
Remove-Item -Path "target" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path "Cargo.lock" -Force -ErrorAction SilentlyContinue
Remove-Item -Path ".sqlx" -Recurse -Force -ErrorAction SilentlyContinue
cargo generate-lockfile
cargo build --release --features redis,observability
```

#### Solución 2: Excluir del Antivirus

Agregar el directorio `target/` a las exclusiones de Windows Defender:

```powershell
# En PowerShell como Administrador
Add-MpPreference -ExclusionPath "C:\ruta\completa\a\tu\proyecto\target"
```

#### Solución 3: Compilar desde WSL (RECOMENDADO - Resuelve 99% de casos)

Si tienes WSL2 instalado, compila desde Linux (evita completamente el problema del linker de Windows):

**Primero, instala las dependencias del sistema:**
```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
```

**Luego compila:**
```bash
cd /mnt/c/ruta/al/proyecto
source ~/.cargo/env
cargo build --release --features redis,observability
```

Ver guía completa en [WSL_COMPILATION.md](WSL_COMPILATION.md).

#### Solución 4: Cambiar Ubicación del Target (Para rutas muy largas)

Mover el directorio `target` fuera del proyecto:

```powershell
# Crear directorio .cargo si no existe
New-Item -ItemType Directory -Force -Path ".cargo"

# Crear archivo .cargo/config.toml
@"
[build]
target-dir = "C:\\temp\\rust-target"
"@ | Out-File -FilePath ".cargo\config.toml" -Encoding UTF8

# Compilar
cargo build --release
```

### Error en WSL: "linker `cc` not found"

**Causa**: Falta el compilador C y dependencias del sistema en WSL.

**Solución**:
```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
```

### Error: "DATABASE_URL must be set"

**Solución**: Configurar la variable de entorno:

```bash
# Windows PowerShell
$env:DATABASE_URL="postgresql://user:pass@localhost:5432/dbname"

# Linux/WSL
export DATABASE_URL="postgresql://user:pass@localhost:5432/dbname"
```

### Error: "Cannot connect to PostgreSQL"

**Solución**: 
1. Verificar que PostgreSQL está corriendo: `docker compose ps` (en `docker_infrastructure/`)
2. Verificar que el puerto 5432 está expuesto: `netstat -an | findstr 5432`
3. Verificar credenciales en `DATABASE_URL`

### Error: "Cannot connect to Redis"

**Solución**:
1. Verificar que Redis está corriendo: `docker compose ps` (en `docker_infrastructure/`)
2. Verificar que el puerto 6379 está expuesto: `netstat -an | findstr 6379`
3. Verificar que la feature `redis` está habilitada: `cargo run --features redis`

## Problemas Post-Migración/Fork

Si el proyecto fue migrado de otro proyecto, pueden quedar artefactos problemáticos:

- **`Cargo.lock` corrupto**: Contiene paths absolutos y referencias al proyecto original
- **`target/` directory**: Cachés de build con rutas del proyecto original
- **Build scripts residuales**: Scripts de build (`build.rs`) con paths hardcodeados
- **Cache de sqlx**: Cachés corruptos de compilación de macros

**Solución completa**: Ver scripts `fix_fork_migration.bat` (Windows) o `fix_fork_migration.sh` (WSL/Linux).

## Recomendación Final

**Para desarrollo en Windows**: Usar WSL2 para compilación (Solución 3). Es la opción más confiable y evita todos los problemas del linker de Windows.

**Para CI/CD**: Usar GitHub Actions con `windows-latest` funciona correctamente (el entorno de CI está optimizado para estos casos).
