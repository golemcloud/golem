# Golem CLI

Command-line interface for interacting with Golem Cloud.

## Features

- Component management
- Worker management
- Agent management
- API key management
- **MCP Server** - Expose Golem CLI as Model Context Protocol server for AI agents

## Installation

```bash
cargo install --path cli/golem-cli
```

## Basic Usage

```bash
# List components
golem-cli component list

# Create a worker
golem-cli worker create --component <component-id> --name <worker-name>

# Start MCP Server
golem-cli mcp-server start --port 3000
```

## MCP Server

The Golem CLI includes an MCP (Model Context Protocol) server that allows AI agents like Claude Code to interact with Golem programmatically.

### Quick Start

```bash
golem-cli mcp-server start --port 3000
```

### Documentation

- **[MCP Server Documentation](MCP_SERVER.md)** - User guide with quick start, available tools, and integration examples
- **[MCP Server Developer Guide](MCP_SERVER_DEV_GUIDE.md)** - Architecture, development workflow, and how to add new tools
- **[MCP Server Quick Reference](MCP_SERVER_QUICK_REF.md)** - Commands, endpoints, and troubleshooting

### Available MCP Tools

- `list_agent_types` - List all available agent types
- `list_components` - List all available components

### Integration Example

```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

## Development

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for development setup and guidelines.

### Build

```bash
cargo build --package golem-cli
```

### Run

```bash
cargo run --package golem-cli -- --help
```

### Test

```bash
cargo test --package golem-cli
```

## Documentation

- [Golem Cloud](https://golem.cloud)
- [Developer Documentation](https://learn.golem.cloud)
- [API Reference](https://docs.golem.cloud)

## License

See [LICENSE](../../LICENSE) in the repository root.
