## Golem CLI MCP – Bounty Evidence Recording Script

This document is a step‑by‑step script to record a single video that shows all required evidence to claim the MCP bounty.

---

## 0. Preparación antes de grabar

- **Entorno:**
  - Estar en el repo `golem` en tu máquina (`C:\Users\...Algora\golem`).
  - Tener `cargo`, `python`, Cursor, Claude Desktop y Gemini CLI instalados.
- **Ventanas recomendadas en pantalla:**
  - Ventana de terminal (PowerShell) en la derecha.
  - Editor (Cursor) con el repo abierto en la izquierda.

---

## 1. Introducción (30–60 segundos)

1. En la grabación, muestra el repo en Cursor.
2. Di algo como:
   - “Este video muestra la implementación del MCP server de `golem-cli`, con transporte HTTP/SSE y stdio, la migración de scripts a Python, y la integración funcionando en Cursor, Claude y Gemini CLI.”
3. Muestra rápidamente estos archivos en el editor:
   - `cli/golem-cli/src/command/mcp_server.rs` (flag `--transport` con default `http`).
   - `cli/golem-cli/src/command_handler/mcp_server.rs` (métodos `run_stdio` y `run_http`).
   - `MCP_STDIO_VS_HTTP.md` (resumen de transporte).

**Objetivo:** Dejar claro que hay soporte dual HTTP/SSE + stdio implementado en el binario oficial.

---

## 2. Build de `golem-cli` (prueba rápida) – Opcional si ya está construido

1. En la terminal, muestra el comando:
   ```bash
   cargo build --release --package golem-cli
   ```
2. No es necesario esperar a que termine si ya está compilado; alcanza con mostrar que se compila sin errores (o mencionar que ya está compilado y mostrar el binario en `target\release\golem-cli.exe`).

**Objetivo:** Mostrar que el binario usado por los clientes MCP proviene de este repo y compila correctamente.

---

## 3. Pruebas automáticas del MCP HTTP/SSE (script Python)

1. En la terminal, arranca el servidor MCP en modo HTTP/SSE:
   ```bash
   target\release\golem-cli.exe mcp-server start --host 127.0.0.1 --port 3000
   ```
   - Deja este servidor corriendo en una terminal.
2. Abre otra terminal y ejecuta:
   ```bash
   python test_mcp_connections.py
   ```
3. Muestra en la grabación la salida del script:
   - `[PASS] Health check`
   - `[PASS] Initialize`
   - `[PASS] List tools`
   - `[PASS] list_agent_types`
   - `[PASS] list_components`
   - `[PASS] Error handling`
   - Al final: `Total: 6/6 tests passed` y `[SUCCESS] All tests passed!`

**Objetivo:** Evidencia clara de que el MCP server HTTP/SSE cumple el protocolo (initialize, tools/list, tools/call, errores).

---

## 4. Prueba manual en Cursor (HTTP/SSE)

### 4.1 Mostrar configuración MCP en Cursor

1. Abre `MCP_CLIENT_CONFIGURATION.md` y señala la sección de Cursor.
2. Muestra el archivo real de configuración:
   - Abre `%APPDATA%\Cursor\User\globalStorage\mcp.json`.
   - Resalta la entrada:
     ```json
     "mcpServers": {
       "golem-cli": {
         "url": "http://127.0.0.1:3000/mcp"
       }
     }
     ```

### 4.2 Probar herramientas con prompts (desde Cursor)

1. Con el servidor HTTP/SSE todavía corriendo:
2. En una conversación de Cursor, lanza estos prompts (también están en `MCP_MANUAL_TESTING_PROMPTS.md`):
   - **Listar herramientas:**
     ```text
     What MCP tools are available from the golem-cli server? Please list all available tools and their descriptions.
     ```
   - **Listar tipos de agentes:**
     ```text
     Use the golem-cli MCP server to list all available agent types in Golem.
     ```
   - **Listar componentes:**
     ```text
     Use the golem-cli MCP server to list all available components in my Golem instance.
     ```
3. En el video:
   - Muestra que Cursor detecta el servidor `golem-cli` como MCP server.
   - Muestra que:
     - Encuentra las herramientas `list_agent_types` y `list_components`.
     - Puede ejecutar esas herramientas y mostrar resultados (o errores esperables si Golem no está configurado).

**Objetivo:** Demostrar integración MCP funcional en Cursor usando HTTP/SSE.

---

## 5. Prueba en Claude Desktop (stdio)

### 5.1 Mostrar configuración MCP de Claude

1. Abre `%APPDATA%\Claude\claude_desktop_config.json`.
2. Resalta la configuración:
   ```json
   "mcpServers": {
     "golem-cli": {
       "command": "C:\\\\...\\\\golem-cli.exe",
       "args": ["mcp-server", "start", "--transport", "stdio"]
     }
   }
   ```
3. Menciona que se generó con `configure_mcp_claude.py`.

### 5.2 Probar herramientas con prompts en Claude Desktop

1. Cierra y vuelve a abrir Claude Desktop antes de grabar esta parte.
2. En una nueva conversación, lanza los mismos prompts clave:
   - Listar herramientas:
     ```text
     What MCP tools are available from the golem-cli server? Please list all available tools and their descriptions.
     ```
   - Listar tipos de agentes:
     ```text
     Use the golem-cli MCP server to list all available agent types in Golem.
     ```
   - Listar componentes:
     ```text
     Use the golem-cli MCP server to list all available components in my Golem instance.
     ```
3. En el video:
   - Explica que Claude arranca `golem-cli mcp-server start --transport stdio` como subprocess.
   - Muestra que las llamadas a herramientas funcionan (o fallan con mensajes esperables si falta configuración de Golem).

**Objetivo:** Demostrar que el mismo servidor MCP funciona en modo stdio para Claude Desktop.

---

## 6. Prueba en Gemini CLI (stdio)

### 6.1 Mostrar configuración de Gemini CLI

1. Abre `C:\Users\...\ .gemini\mcp_config.json` (según la ruta creada).
2. Resalta:
   ```json
   "mcpServers": {
     "golem-cli": {
       "command": "C:\\\\...\\\\golem-cli.exe",
       "args": ["mcp-server", "start", "--transport", "stdio"]
     }
   }
   ```
3. Menciona que se generó con `configure_mcp_gemini.py`.

### 6.2 Probar prompts desde Gemini CLI

1. Abre Gemini CLI.
2. Lanza prompts similares:
   - Listar herramientas:
     ```text
     What MCP tools are available from the golem-cli server? Please list all available tools and their descriptions.
     ```
   - Listar tipos de agentes:
     ```text
     Use the golem-cli MCP server to list all available agent types in Golem.
     ```
   - Listar componentes:
     ```text
     Use the golem-cli MCP server to list all available components in my Golem instance.
     ```
3. En el video:
   - Muestra que el CLI usa el servidor stdio y obtiene respuestas razonables.

**Objetivo:** Evidencia de integración MCP funcional en un tercer cliente stdio (Gemini CLI).

---

## 7. Evidencia de migración a Python y limpieza de scripts antiguos

1. En el editor, muestra:
   - `configure_mcp_cursor.py`
   - `configure_mcp_claude.py`
   - `configure_mcp_gemini.py`
   - `test_mcp_manual.py`, `test_mcp_final.py`, etc. (scripts Python existentes).
2. Muestra que los scripts PowerShell/BAT antiguos están eliminados:
   - Usa una búsqueda rápida en el repo (`test_mcp_*.ps1`, `.bat`) mostrando que no existen o enseñando la lista limpia.
3. Opcional: ejecuta en la terminal:
   ```bash
   python -m py_compile configure_mcp_cursor.py configure_mcp_claude.py configure_mcp_gemini.py
   ```
   y muestra que no hay errores.

**Objetivo:** Probar que toda la automatización y configuración está ahora en Python, sin PowerShell/BAT.

---

## 8. Cierre del video

1. Resume en voz alta:
   - “El MCP server de `golem-cli` soporta HTTP/SSE y stdio.”
   - “Hemos probado el protocolo con tests automáticos en Python.”
   - “Hemos configurado y probado el MCP en Cursor (HTTP/SSE), Claude Desktop (stdio) y Gemini CLI (stdio) usando los mismos tools (`list_agent_types`, `list_components`).”
   - “Todos los scripts auxiliares fueron migrados a Python.”
2. Muestra brevemente:
   - `MCP_MANUAL_TESTING_PROMPTS.md`
   - `MCP_CLIENT_CONFIGURATION.md`
   - `MCP_STDIO_VS_HTTP.md`
3. Termina con:
   - “Con esto, el bounty queda completo y listo para revisión.”

---

## Resumen rápido para grabar (checklist)

- [ ] Mostrar código del servidor MCP (HTTP/SSE + stdio).
- [ ] Correr `python test_mcp_connections.py` con todos los tests en verde.
- [ ] Mostrar Cursor configurado y funcionando con prompts.
- [ ] Mostrar Claude Desktop usando MCP stdio con prompts.
- [ ] Mostrar Gemini CLI usando MCP stdio con prompts.
- [ ] Mostrar scripts Python de configuración y ausencia de PowerShell/BAT.
- [ ] Hacer resumen final verbal de todo lo demostrado.

