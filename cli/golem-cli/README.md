# Basic MCP server

Basic options:

- [x] **HTTP transport**: You can `curl` `http://127.0.0.1:1232/mcp` with JSON-RPC requests (not stdio).
- [x] **MCP methods implemented**: `initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`.
- [x] **Incremental output**: `tools/call` returns a `logs` array with interleaved `stdout`/`stderr` lines.
- [x] **Safety**: Disallowed top-level subcommands are blocked with a clear error.
- [x] **Resources**: `resources/list` shows manifest files from the **current dir, ancestors, and immediate children**; `resources/read` returns file contents.
- [x] **No runtime panics** under normal use; you can run `version`, `profile list`, etc.

---

# 🔬 MCP Server Test Plan for golem-cli

Make sure the server is running:

```bash
./target/release/golem-cli --serve --serve-port 1232
```

# 0) Health check (server up?)
```bash
curl -sS -o /dev/null -w "%{http_code}
" http://127.0.0.1:1232/mcp
```
> Expected: 405

# 1) initialize
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | jq
```
> Expected: { "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": "...", "serverInfo": {...}, "capabilities": {...} } }

# 2) tools/list
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | jq
```
> Expected: one tool: "golem.run" with args + cwd schema

# 3) tools/call — happy path
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{
        "jsonrpc":"2.0",
        "id":"v1",
        "method":"tools/call",
        "params":{
          "name":"golem.run",
          "arguments":{"args":["version"]}
        }
      }' | jq
```
> Expected: { "ok": true, "command": {"binary":"golem","args":["version"]}, "logs":[...], "result":{"exitCode":0} }

# 4) tools/call — disallowed subcommand
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{
        "jsonrpc":"2.0",
        "id":"bad",
        "method":"tools/call",
        "params":{
          "name":"golem.run",
          "arguments":{"args":["system","exec","rm","-rf","/"]}
        }
      }' | jq
```
> Expected: error with "Disallowed subcommand 'system'"

# 5) tools/call — with cwd
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{
        "jsonrpc":"2.0",
        "id":"cwd",
        "method":"tools/call",
        "params":{
          "name":"golem.run",
          "arguments":{"args":["profile","list"], "cwd":"/tmp"}
        }
      }' | jq
```
> Expected: same shape as happy path but in /tmp

# 6) resources/list
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":"rlist","method":"resources/list","params":{}}' | jq
```
> Expected: list of files (e.g. manifest.yaml)

# 7) resources/read
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d "{
        \"jsonrpc\":\"2.0\",
        \"id\":\"rread\",
        \"method\":\"resources/read\",
        \"params\":{
          \"uri\":\"file:///abs/path/to/manifest.yaml\"
        }
      }" | jq
```
> Expected: { "contents": [ { "uri": "...", "mimeType":"application/yaml", "text": "..." } ] }

# 8a) Error case: Unknown method
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":"x","method":"nonsense","params":{}}' | jq
```
> Expected: error { "code": -32601, "message": "Method not found" }

# 8b) Error case: Wrong tool name
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":"badtool","method":"tools/call","params":{"name":"not-a-tool","arguments":{}}}' | jq
```
> Expected: error { "code": -32602, "message": "Unknown tool name" }

# 8c) Error case: Bad URI
```bash
curl -sS http://127.0.0.1:1232/mcp   -H 'content-type: application/json'   -d '{"jsonrpc":"2.0","id":"baduri","method":"resources/read","params":{"uri":"notfile:///tmp/foo"}}' | jq
```
> Expected: error { "code": -32602, "message": "Only file:// URIs are supported" }
