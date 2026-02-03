# MCP Transport: stdio vs HTTP - Problema Resuelto

## ✅ Problema Resuelto

**Claude Desktop espera**: Servidores MCP que usen **stdio** (stdin/stdout)  
**golem-cli ahora soporta**: Ambos modos - **HTTP/SSE** y **stdio**

### Solución Implementada

El servidor MCP de `golem-cli` ahora soporta **ambos modos de transporte**:
1. **HTTP mode** (default): Para clientes HTTP como Claude Code
2. **Stdio mode**: Para clientes stdio como Claude Desktop

---

## Uso

### HTTP Mode (default)

```bash
golem-cli mcp-server start --host 127.0.0.1 --port 3000
```

### Stdio Mode (para Claude Desktop)

```bash
golem-cli mcp-server start --transport stdio
```

### Opciones del Comando

- `--transport` - Modo de transporte: `http` (default) o `stdio`
- `--host` - Dirección del host (solo modo HTTP, default: `127.0.0.1`)
- `--port` - Puerto (solo modo HTTP, default: `3000`)

---

## Configuración

### Claude Desktop (stdio)

```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

### Claude Code / HTTP Clients (HTTP)

```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

---

## Historial de Soluciones (Obsoleto - Ya Implementado)

### ~~Opción 1: Agregar Soporte stdio a golem-cli (Recomendado)~~ ✅ IMPLEMENTADO

**Ventajas:**
- Solución nativa y estándar
- Compatible con Claude Desktop y otros clientes MCP
- Mantiene soporte HTTP también (dual mode)
- No requiere procesos adicionales

**Desventajas:**
- Requiere modificar código fuente
- Requiere recompilar el binario

**Implementación:**

1. Agregar flag `--transport` con opciones: `http` (default) o `stdio`
2. Usar `rmcp::transport::io` para modo stdio
3. Mantener `StreamableHttpService` para modo HTTP

**Código necesario:**

```rust
// En src/command/mcp_server.rs
#[derive(Debug, Clone, Args)]
pub struct McpServerStartArgs {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    
    #[arg(long, default_value_t = 3000)]
    pub port: u16,
    
    // NUEVO: Transport mode
    #[arg(long, default_value = "http", value_parser = ["http", "stdio"])]
    pub transport: String,
}

// En src/command_handler/mcp_server.rs
impl McpServerCommandHandlerDefault {
    async fn run(&self, args: McpServerStartArgs) -> Result<()> {
        let service = McpServerImpl::new(self.ctx.clone());
        
        match args.transport.as_str() {
            "stdio" => {
                // Usar stdio transport
                use rmcp::transport::io::StdioTransport;
                use tokio::io::{stdin, stdout};
                
                let stdin = stdin();
                let stdout = stdout();
                let transport = StdioTransport::new(stdin, stdout);
                
                // Ejecutar servidor en modo stdio
                rmcp::run_stdio_server(service, transport).await?;
            }
            "http" | _ => {
                // Código HTTP existente
                let addr = format!("{}:{}", args.host, args.port);
                logln(format!("Starting MCP server on {}", addr));
                
                let mcp_service = StreamableHttpService::new(
                    move || Ok(service.clone()),
                    LocalSessionManager::default().into(),
                    Default::default(),
                );
                
                let app = Router::new()
                    .nest_service("/mcp", mcp_service)
                    .route("/", get(|| async { "Hello from Golem CLI MCP Server!" }));
                
                let listener = tokio::net::TcpListener::bind(addr).await?;
                axum::serve(listener, app).await?;
            }
        }
        
        Ok(())
    }
}
```

**Nota:** Esta implementación requiere verificar la API exacta de `rmcp::transport::io`. La biblioteca `rmcp` tiene `transport-io` como feature, pero la API puede variar.

---

### Opción 2: Crear un Bridge stdio-to-HTTP

**Ventajas:**
- No requiere modificar golem-cli
- Rápido de implementar
- Separación de responsabilidades

**Desventajas:**
- Proceso adicional (overhead)
- Más complejo de mantener
- Posibles problemas de sincronización

**Implementación con Python:**

```python
#!/usr/bin/env python3
"""Bridge stdio-to-HTTP para golem-cli MCP server"""
import sys
import json
import subprocess
import requests
import time
import threading

SERVER_URL = "http://127.0.0.1:3000/mcp"
SERVER_PROCESS = None

def start_server():
    """Inicia el servidor HTTP en background"""
    global SERVER_PROCESS
    SERVER_PROCESS = subprocess.Popen(
        ["golem-cli", "mcp-server", "start", "--host", "127.0.0.1", "--port", "3000"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL
    )
    # Esperar a que el servidor esté listo
    for _ in range(20):
        try:
            requests.get("http://127.0.0.1:3000/", timeout=0.1)
            return True
        except:
            time.sleep(0.1)
    return False

def forward_request(request_json: str) -> str:
    """Envía request al servidor HTTP y devuelve respuesta"""
    try:
        response = requests.post(
            SERVER_URL,
            headers={"Content-Type": "application/json"},
            data=request_json,
            timeout=5
        )
        return response.text
    except Exception as e:
        return json.dumps({
            "jsonrpc": "2.0",
            "id": None,
            "error": {
                "code": -32603,
                "message": f"Bridge error: {str(e)}"
            }
        })

def main():
    # Iniciar servidor HTTP
    if not start_server():
        sys.stderr.write("ERROR: Failed to start HTTP server\n")
        sys.exit(1)
    
    try:
        # Leer desde stdin línea por línea
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            
            # Enviar request al servidor HTTP
            response = forward_request(line)
            
            # Escribir respuesta a stdout
            sys.stdout.write(response + "\n")
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass
    finally:
        # Terminar servidor
        if SERVER_PROCESS:
            SERVER_PROCESS.terminate()
            SERVER_PROCESS.wait()

if __name__ == "__main__":
    main()
```

**Implementación con Rust (más eficiente):**

```rust
// cli/mcp-stdio-bridge/src/main.rs
use std::process::{Command, Stdio};
use std::io::{self, BufRead, Write};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Iniciar servidor HTTP
    let mut server = TokioCommand::new("golem-cli")
        .args(&["mcp-server", "start", "--host", "127.0.0.1", "--port", "3000"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    
    // Esperar a que el servidor esté listo
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    let client = reqwest::Client::new();
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = io::BufReader::new(stdin);
    let mut lines = reader.lines();
    
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        
        // Enviar request al servidor HTTP
        let response = client
            .post("http://127.0.0.1:3000/mcp")
            .header("Content-Type", "application/json")
            .body(line)
            .send()
            .await?;
        
        let response_text = response.text().await?;
        
        // Escribir respuesta a stdout
        stdout.write_all(response_text.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    
    server.kill().await?;
    Ok(())
}
```

**Configuración de Claude Desktop:**

```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "python",
      "args": ["path/to/mcp_stdio_bridge.py"]
    }
  }
}
```

---

### Opción 3: Usar un Cliente MCP que Soporte HTTP

**Ventajas:**
- No requiere cambios en golem-cli
- Solución inmediata

**Desventajas:**
- Claude Desktop no soporta HTTP directamente
- Requiere usar otro cliente MCP

**Clientes MCP que soportan HTTP:**
- Claude Code (puede que soporte HTTP)
- Clientes MCP personalizados
- Integración directa vía HTTP API

---

## Recomendación

**Opción 1 (Agregar soporte stdio)** es la mejor solución a largo plazo porque:
1. Es el estándar MCP
2. Compatible con Claude Desktop
3. Permite mantener ambos modos (HTTP y stdio)
4. No requiere procesos adicionales
5. Mejor performance

**Opción 2 (Bridge)** es una solución temporal rápida si:
1. No puedes modificar golem-cli inmediatamente
2. Necesitas compatibilidad con Claude Desktop ahora
3. Puedes mantener un proceso adicional

---

## Estado Actual

El servidor MCP de `golem-cli` actualmente:
- ✅ Soporta HTTP/SSE transport
- ✅ Soporta stdio transport
- ✅ Es compatible con Claude Desktop directamente

Para usar con Claude Desktop, simplemente configura el servidor con `--transport stdio` como se muestra en la sección de Configuración arriba.

---

## Verificación

El código ha sido verificado y compila correctamente:
- ✅ Feature `transport-io` habilitada en `Cargo.toml`
- ✅ Implementación de `run_stdio()` usando `rmcp::transport::io::stdio()`
- ✅ Flag `--transport` con opciones `http` y `stdio` implementado
- ✅ Código compila sin errores

---

## Referencias

- [MCP Specification - Transport](https://spec.modelcontextprotocol.io/transport)
- [rmcp GitHub Repository](https://github.com/rust-mcp-stack/rust-mcp-sdk)
- [Claude Desktop MCP Documentation](https://claude.ai/docs/mcp)
