# WSL Quick Start - Compilación del SDK

Guía rápida para compilar el SDK desde WSL cuando hay problemas con el linker de Windows.

## Instalación de Rust en WSL

Si Rust no está instalado en WSL:

```bash
# Desde WSL (no desde PowerShell)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Seguir las instrucciones (presionar Enter para instalación por defecto)

# Recargar el entorno
source ~/.cargo/env

# Verificar instalación
rustc --version
cargo --version
```

## Compilar el Proyecto

Una vez que Rust está instalado:

```bash
# 1. Navegar al proyecto desde WSL
cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT

# 2. Configurar variables de entorno
export DATABASE_URL="postgresql://mig_topology_user:mig_topology_pass@localhost:5432/mig_topology"
export REDIS_URL="redis://localhost:6379"

# Si tienes un .env, puedes cargarlo:
# source <(grep -v '^#' docker_infrastructure/.env | sed 's/^/export /' | grep -E '^(DATABASE_URL|REDIS_URL|SDK_RPC)')

# 3. Compilar (esto evitará el problema del linker de Windows)
cargo build --features redis,observability

# O verificar compilación sin compilar completamente:
cargo check --features redis,observability
```

## Ejecutar el SDK

Una vez compilado:

```bash
# Ejecutar ejemplo básico
cargo run --example basic_setup --features redis,observability

# O ejecutar en modo release (más rápido):
cargo run --release --example basic_setup --features redis,observability
```

## Notas Importantes

- **Desde WSL, `localhost` apunta al host de Windows**, así que puedes conectarte a PostgreSQL y Redis que están corriendo en Docker en Windows
- Los puertos 5432 (PostgreSQL) y 6379 (Redis) deben estar expuestos en `0.0.0.0` (ya configurado en docker-compose.yml)
- La primera compilación puede tomar varios minutos (descarga y compila todas las dependencias)

## Troubleshooting

**Error: "curl: command not found"**
```bash
sudo apt-get update
sudo apt-get install -y curl
```

**Error: "Permission denied"**
- No necesitas sudo para instalar Rust (se instala en tu home directory)
- Si tienes problemas de permisos, verifica que estés en tu directorio home

**Error de conexión a PostgreSQL/Redis**
- Asegúrate de que los servicios Docker estén corriendo en Windows
- Desde PowerShell: `cd docker_infrastructure && docker compose ps`
- Verifica que los puertos estén expuestos: `netstat -an | findstr 5432`

**Compilación muy lenta**
- Compilar desde `/mnt/c/` (filesystem de Windows) es más lento
- Para desarrollo frecuente, considera copiar el proyecto al filesystem de WSL
- O usar `cargo check` en lugar de `cargo build` durante desarrollo
