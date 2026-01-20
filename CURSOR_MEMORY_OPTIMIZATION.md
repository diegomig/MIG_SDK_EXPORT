# Estrategias para Reducir Uso de Memoria en Cursor

## ⚠️ PROBLEMA CRÍTICO IDENTIFICADO

**Causa raíz**: El directorio `target/` tiene **14GB** de archivos de build de Rust. Cursor está indexando todos estos archivos, causando OOM.

**Solución inmediata**: Excluir `target/` del indexado de Cursor.

---

## 🚨 ACCIONES INMEDIATAS REQUERIDAS

### 1. Crear `.cursorignore` en la raíz del proyecto
```bash
# Crear archivo .cursorignore con este contenido:
target/
**/target/
*.so
*.dylib
*.dll
*.exe
target/debug/
target/release/
.cargo/registry/
*.log
*.jsonl
```

### 2. Limpiar directorio target (OPCIONAL - puede tardar)
```bash
wsl bash -c "cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT && /home/miga/.cargo/bin/cargo clean"
```
**Nota**: Este comando puede tardar varios minutos con 14GB. Es opcional si ya excluimos `target/` del indexado.

### 3. Reiniciar Cursor
Después de crear `.cursorignore`, reiniciar Cursor para que tome efecto.

---

## ✅ Estrategias Implementadas

### 1. Uso Exclusivo de WSL
- ✅ Todos los comandos se ejecutan en WSL (`wsl bash -c`)
- ✅ Evita procesos duplicados en Windows
- ✅ Reduce carga de memoria del sistema

### 2. Comandos Eficientes
- ✅ Usar `grep` para filtrar errores específicos en lugar de leer archivos completos
- ✅ Usar `tail` y `head` para limitar salida de comandos
- ✅ Evitar leer múltiples archivos grandes simultáneamente
- ✅ **NUEVO**: Limitar lectura de archivos a máximo 100 líneas por vez

### 3. Lectura Selectiva de Archivos
- ✅ Leer solo secciones específicas con `offset` y `limit`
- ✅ Usar `grep` para buscar patrones antes de leer archivos completos
- ✅ Evitar leer archivos de log grandes
- ✅ **NUEVO**: Nunca leer archivos >1MB completos

### 4. Exclusión de Directorios Grandes
- ✅ Crear `.cursorignore` para excluir `target/` (14GB)
- ✅ Excluir archivos binarios grandes (*.so, *.dylib, *.exe)
- ✅ Excluir logs y archivos temporales

### 5. Configuración de Cursor (Recomendaciones)
- Reducir tamaño de contexto del modelo si es posible
- Cerrar pestañas no usadas
- Desactivar extensiones innecesarias
- Reiniciar Cursor periódicamente
- **NUEVO**: Verificar que `.cursorignore` esté funcionando

### 6. Optimización de Código
- ✅ Evitar estructuras de datos muy grandes en memoria
- ✅ Usar referencias en lugar de clones cuando sea posible
- ✅ Limitar tamaño de buffers y cachés

## Comandos Útiles

### Verificar uso de memoria del proceso
```bash
# En WSL
ps aux | grep -i cursor | awk '{print $2, $4, $11}'
```

### Limpiar build artifacts
```bash
wsl bash -c "cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT && cargo clean"
```

### Verificar tamaño de archivos grandes
```bash
wsl bash -c "cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT && find . -type f -size +1M -exec ls -lh {} \; | head -20"
```

## 📊 Estado Actual

- ✅ `.cursorignore` creado - Excluye `target/` (14GB)
- ⚠️ `target/` aún existe pero ahora está excluido del indexado
- ✅ Estrategias de lectura eficiente implementadas

## 🔧 Cambios en Flujo de Trabajo

### Antes (causaba OOM):
- Leer archivos completos grandes
- Múltiples lecturas simultáneas
- Sin exclusión de directorios grandes

### Ahora (optimizado):
- ✅ Leer máximo 100-200 líneas por archivo
- ✅ Usar `grep` antes de leer archivos
- ✅ `.cursorignore` excluye 14GB de `target/`
- ✅ Comandos específicos en lugar de búsquedas amplias

## ⚡ Próximos Pasos

1. **INMEDIATO**: Limpiar `target/` completamente (ejecutando `cargo clean` en background)
2. **INMEDIATO**: Reiniciar Cursor para que `.cursorignore` actualizado tome efecto
3. Monitorear si los crashes OOM se reducen
4. Si persisten después de limpiar `target/`:
   - Reducir tamaño de workspace (mover archivos grandes fuera)
   - Cerrar todas las pestañas no usadas
   - Desactivar extensiones innecesarias
   - Considerar trabajar en subdirectorios específicos

## 🚨 ACCIÓN URGENTE: Limpiar target/

El comando `cargo clean` está ejecutándose en background. Esto eliminará los 1.4GB de `target/`.

**Después de que termine:**
1. Verificar: `du -sh target/` debería mostrar ~0 o muy pequeño
2. Reiniciar Cursor completamente
3. El `.cursorignore` expandido ahora excluye más archivos

## 🎯 Verificación

Para verificar que `.cursorignore` está funcionando:
1. Reiniciar Cursor
2. Abrir Command Palette (Ctrl+Shift+P)
3. Buscar archivos en `target/` - deberían estar excluidos
4. Verificar que el uso de memoria se reduce
