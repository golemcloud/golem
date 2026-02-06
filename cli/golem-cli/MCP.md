# Golem MCP Server

The Golem CLI ships an MCP (Model Context Protocol) server that exposes Golem
Cloud operations as tools and resources over stdio. This lets AI assistants
(Claude Desktop, VS Code MCP, etc.) call Golem APIs through the CLI.

## Setup

Environment variables:
- `GOLEM_API_KEY`: your Golem API token (required)
- `GOLEM_API_URL`: API base URL (optional, default `https://api.golem.cloud`)

Start the server (canonical path):
```
golem-cli --serve --serve-port 1232
```

Alias:
```
golem-cli mcp serve --serve-port 1232
```

The server listens on `http://localhost:<PORT>/mcp`.

## Tools

- `golem_list_workers`: list workers for a component
- `golem_get_worker`: get worker metadata
- `golem_invoke_worker`: invoke a worker function
- `golem_create_worker`: create a worker
- `golem_delete_worker`: delete a worker
- `golem_list_components`: list components for an environment
- `golem_get_deployment`: get deployment summary

## Resources

The server exposes manifest files discovered from:
- current working directory
- ancestor directories
- direct child directories

Manifest filenames:
- `golem.yaml`
- `golem.yml`

Each manifest is exposed as a `file://` resource URI.

## Claude Desktop configuration

Add a server entry (example):
```
{
  "mcpServers": {
    "golem": {
      "command": "golem-cli",
      "args": ["--serve", "--serve-port", "1232"],
      "env": {
        "GOLEM_API_KEY": "YOUR_TOKEN",
        "GOLEM_API_URL": "https://api.golem.cloud"
      }
    }
  }
}
```
