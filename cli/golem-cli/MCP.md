# Golem MCP Server

The Golem CLI ships an MCP (Model Context Protocol) server that exposes Golem
Cloud operations as tools and resources over stdio. This lets AI assistants
(Claude Desktop, VS Code MCP, etc.) call Golem APIs through the CLI.

## Setup

Environment variables:
- `GOLEM_API_KEY`: your Golem API token (required)
- `GOLEM_API_URL`: API base URL (optional, default `https://api.golem.cloud`)

Start the server:
```
golem-cli mcp serve
```

## Tools

- `golem_list_workers`: list workers for a component
- `golem_get_worker`: get worker metadata
- `golem_invoke_worker`: invoke a worker function
- `golem_create_worker`: create a worker
- `golem_delete_worker`: delete a worker
- `golem_list_components`: list components for an environment
- `golem_get_deployment`: get deployment summary

## Resources

Resource templates:
- `golem://components/{environment_id}`
- `golem://workers/{component_id}`
- `golem://workers/{component_id}/{worker_name}/oplog{?from,count,query}`
- `golem://deployments/{environment_id}`
- `golem://deployments/{environment_id}/current`
- `golem://deployments/{environment_id}/{deployment_id}`

## Claude Desktop configuration

Add a server entry (example):
```
{
  "mcpServers": {
    "golem": {
      "command": "golem-cli",
      "args": ["mcp", "serve"],
      "env": {
        "GOLEM_API_KEY": "YOUR_TOKEN",
        "GOLEM_API_URL": "https://api.golem.cloud"
      }
    }
  }
}
```
