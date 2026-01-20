# Compilación y Ejecución desde WSL

Esta guía explica cómo compilar y ejecutar el SDK desde WSL (Windows Subsystem for Linux) cuando se encuentran problemas con el linker de Windows.

## Prerrequisitos

1. **WSL instalado y funcionando** (Ubuntu recomendado)
2. **Rust instalado en WSL** (si no está, ver abajo)
3. **Servicios Docker corriendo en Windows** (PostgreSQL y Redis)
4. **RPC configurado** en `.env` o variables de entorno

## Instalación de Rust en WSL (si es necesario)

```bash
# Desde WSL
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustc --version  # Verificar instalación
```

## Paso 1: Acceder al Proyecto desde WSL

El proyecto está en la ruta de Windows, accesible desde WSL en `/mnt/c/`:

```bash
cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT
```

## Paso 2: Verificar Servicios Docker

Los servicios Docker deben estar corriendo en Windows. Desde PowerShell/CMD en Windows:

```powershell
cd docker_infrastructure
docker compose ps
```

Debes ver:
- `mig-topology-postgres` - Up (healthy) - 0.0.0.0:5432->5432/tcp
- `mig-topology-redis` - Up (healthy) - 0.0.0.0:6379->6379/tcp

## Paso 3: Configurar Variables de Entorno

Desde WSL, configurar las variables de entorno. **Importante**: Desde WSL, `localhost` apunta al host de Windows, así que puedes conectarte a los servicios Docker.

```bash
# Desde WSL, en el directorio del proyecto
export DATABASE_URL="postgresql://mig_topology_user:mig_topology_pass@localhost:5432/mig_topology"
export REDIS_URL="redis://localhost:6379"

# RPC Configuration (desde tu .env o configurar manualmente)
# Si tienes un .env en docker_infrastructure, puedes cargarlo:
# source <(grep -v '^#' docker_infrastructure/.env | sed 's/^/export /')

# O configurar manualmente:
export SDK_RPC_HTTP_URLS="https://arb1.arbitrum.io/rpc"
export SDK_RPC_WS_URLS="wss://arb1.arbitrum.io/ws"

# Logging
export RUST_LOG=info
export RUST_BACKTRACE=1
```

## Paso 4: Compilar desde WSL

```bash
# Compilar el proyecto (esto evitará el problema del linker de Windows)
cargo build --features redis,observability

# O solo verificar compilación:
cargo check --features redis,observability
```

## Paso 5: Ejecutar el SDK

Una vez compilado, puedes ejecutar los ejemplos:

```bash
# Ejemplo básico de setup
cargo run --example basic_setup --features redis,observability

# Otros ejemplos disponibles:
# cargo run --example realtime_updates --features redis,observability
# cargo run --example liquidity_path --features redis,observability
```

## Paso 6: Generar Métricas y Benchmarks

Para generar métricas con el Flight Recorder y benchmarks:

1. **Asegúrate de que el Flight Recorder esté habilitado** en el código (normalmente se activa via variable de entorno o código)

2. **Ejecuta el SDK** con las features de observabilidad:

```bash
cargo run --example basic_setup --features redis,observability --release
```

3. **Los logs y métricas** se generarán según la configuración del Flight Recorder

## Notas Importantes

### Conexión a Docker desde WSL

- Desde WSL, `localhost` se mapea al host de Windows
- Los puertos 5432 (PostgreSQL) y 6379 (Redis) están expuestos en `0.0.0.0`, así que son accesibles desde WSL
- No necesitas cambiar las URLs de conexión, `localhost` funcionará correctamente

### Performance

- Compilar desde WSL puede ser más lento que desde Windows nativo (por el sistema de archivos montado)
- Para compilaciones más rápidas, considera compilar en WSL pero con el código en el filesystem de WSL (copiando el proyecto)
- Para desarrollo, la compilación desde `/mnt/c/` es aceptable

### Troubleshooting

**Error de conexión a PostgreSQL/Redis:**
```bash
# Verificar que los servicios están corriendo desde Windows
# Verificar que los puertos están expuestos correctamente
netstat -an | grep 5432  # Desde Windows PowerShell
```

**Error de compilación:**
- Asegúrate de tener todas las dependencias del sistema instaladas en WSL:
  ```bash
  sudo apt-get update
  sudo apt-get install build-essential pkg-config libssl-dev
  ```

**RPC no accesible:**
- Verifica que las URLs del RPC estén correctamente configuradas
- Asegúrate de que WSL tenga acceso a internet

## Alternativa: Compilar en WSL, Ejecutar Binario desde Windows

Si prefieres compilar en WSL pero ejecutar desde Windows (por performance o compatibilidad):

1. Compilar desde WSL (como se muestra arriba)
2. El binario estará en: `target/release/` o `target/debug/`
3. Desde Windows PowerShell, ejecutar el binario directamente:
   ```powershell
   .\target\release\examples\basic_setup.exe
   ```

Sin embargo, esto requiere compilar para el target de Windows desde WSL, lo cual es más complejo. La opción más simple es compilar y ejecutar todo desde WSL.
