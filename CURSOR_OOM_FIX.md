# 🚨 Solución Urgente: Cursor OOM Crashes

## ✅ ACCIONES COMPLETADAS

1. ✅ **`.cursorignore` expandido** - Ahora excluye:
   - `target/` completo (1.3GB)
   - Archivos binarios grandes (*.so, *.dll, *.exe, *.rlib)
   - Logs y temporales (*.log, *.jsonl, *.tmp)
   - Directorios de IDE (.vscode/, .idea/)
   - Flight recorder logs

2. ✅ **`cargo clean` ejecutado** - Limpiando `target/` (1.3GB → ~0)

## 🔄 ACCIÓN REQUERIDA AHORA

### Paso 1: Verificar limpieza de target/
```bash
wsl bash -c "cd /mnt/c/Users/54223/Documents/01_ai/MIG_SDK_EXPORT && du -sh target/"
```
**Esperado**: Debería mostrar <100MB o "no existe"

### Paso 2: Reiniciar Cursor COMPLETAMENTE
1. **Cerrar TODAS las ventanas de Cursor**
2. **Cerrar el proceso desde Task Manager** (si es necesario)
3. **Esperar 10 segundos**
4. **Abrir Cursor nuevamente**

### Paso 3: Verificar que `.cursorignore` funciona
1. Abrir Command Palette (Ctrl+Shift+P)
2. Buscar "Files: Exclude"
3. Verificar que `target/` está en la lista de exclusiones

## 📊 Estado del Proyecto

- **target/**: 1.3GB → Limpiándose (debería quedar ~0)
- **src/**: 1.5MB ✅
- **Otros directorios**: <500KB cada uno ✅

## ⚠️ Si los Crashes Persisten

### Opción A: Reducir Workspace
Trabajar solo en subdirectorios específicos:
- Abrir solo `src/` en lugar del proyecto completo
- Usar "File > Add Folder to Workspace" selectivamente

### Opción B: Configuración de Cursor
1. Settings → Search "files.exclude"
2. Agregar manualmente:
   ```
   **/target/**
   **/*.so
   **/*.dll
   **/*.exe
   **/*.log
   **/*.jsonl
   ```

### Opción C: Limitar Memoria
1. Settings → Search "memory"
2. Reducir "max memory" si existe la opción
3. Desactivar extensiones innecesarias

## 🎯 Verificación Post-Reinicio

Después de reiniciar Cursor:
1. ✅ No debería indexar `target/`
2. ✅ Uso de memoria debería ser menor
3. ✅ No debería crashear por OOM

## 📝 Notas

- El `.cursorignore` está en la raíz del proyecto
- `cargo clean` elimina todos los artifacts de build
- Los artifacts se regenerarán al compilar, pero Cursor los ignorará
