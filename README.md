# Golem

![Golem Logo](golem-logo-black.jpg)

This repository contains Golem - a set of services enable you to run WebAssembly components in a distributed cloud environment.

## Getting started with Golem
See [Golem Cloud](https://golem.cloud) for more information, and [the Golem Developer Documentation](https://learn.golem.cloud) for getting started.

## MCP Server Mode

The Golem CLI can run as an MCP (Model Context Protocol) server, enabling AI agents and tools to interact with Golem programmatically.

### Starting the MCP Server

```bash
golem-cli --serve --serve-port 1232
```

### Features

- **Tools**: All CLI commands are exposed as MCP tools. AI agents can execute any CLI command by calling the corresponding tool with arguments.
- **Resources**: `golem.yaml` manifest files from the current directory, parent directories, and child directories are exposed as MCP resources.

### Example Usage

```bash
# Initialize MCP session
curl -X POST http://127.0.0.1:1232/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"my-client","version":"1.0"}}}'

# List available tools
curl -X POST http://127.0.0.1:1232/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

# Call a tool (e.g., get help for the `component` command)
curl -X POST http://127.0.0.1:1232/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"golem-cli.component","arguments":{"args":["--help"]}}}'
```

## Developing Golem
Find details in the [contribution guide](CONTRIBUTING.md) about how to compile the Golem services locally.
