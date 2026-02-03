# Populating Test Data for MCP Server

The MCP tools return empty results when there's no data in your Golem instance. Follow these steps to populate test data.

## Quick Start

Run the automated script:

```bash
python populate_test_data.py
```

## Manual Steps

### 1. Authenticate with Golem Cloud

```bash
golem-cli --profile cloud cloud account get
```

This will open a browser for OAuth authentication.

### 2. Deploy Example Component

Navigate to the example app and deploy:

```bash
cd rust-deploy
golem-cli --profile cloud app build
golem-cli --profile cloud app deploy -E cloud
```

Or use the shopping-app example:

```bash
cd cli/golem-cli/shopping-app
golem-cli --profile cloud app build
golem-cli --profile cloud app deploy -E cloud
```

### 3. Create Test Workers

After deploying a component, create some workers:

```bash
# List components to get the component name
golem-cli --profile cloud component list

# Create workers (replace COMPONENT_NAME with actual name)
golem-cli --profile cloud worker add --component COMPONENT_NAME --worker-name test-worker-1
golem-cli --profile cloud worker add --component COMPONENT_NAME --worker-name test-worker-2
golem-cli --profile cloud worker add --component COMPONENT_NAME --worker-name test-worker-3
```

### 4. Register Agent Types

Agent types are registered through:

1. **Golem Cloud Console**: Visit https://release.golem.cloud and navigate to Agents section
2. **Component Metadata**: Agent types can be defined in component WIT files
3. **API Calls**: Use Golem API to register agent types programmatically

Example agent type registration (if supported):

```bash
golem-cli --profile cloud app register-agent-type \
  --name "shopping-assistant" \
  --description "Handles shopping cart operations"
```

### 5. Verify Data

Check that data was populated:

```bash
# List components
golem-cli --profile cloud component list

# List workers
golem-cli --profile cloud worker list

# List agent types
golem-cli --profile cloud app list-agent-types
```

## Test MCP Tools

Once data is populated, test the MCP tools:

### Using Python Test Script

```bash
python test_mcp_e2e_full.py
```

### Manual MCP Test

```bash
# Start MCP server
golem-cli mcp-server start --transport stdio

# In another terminal, run:
python test_mcp_manual.py
```

### Using Cursor/Claude

1. Configure MCP in `~/.cursor/mcp.json` or Claude Desktop config
2. Reload the window
3. Use @ to reference golem-cli tools
4. Ask: "List all Golem components"

## Expected Results

After populating data, MCP tools should return:

### list_components
```json
{
  "components": [
    {
      "id": "...",
      "name": "deploy:counter",
      "revision": 0,
      "size": 12345
    }
  ]
}
```

### list_workers
```json
{
  "workers": [
    {
      "worker_id": "...",
      "component_name": "deploy:counter",
      "status": "Idle",
      "created_at": "2026-02-03T00:00:00Z"
    }
  ]
}
```

### list_agent_types
```json
{
  "agent_types": [
    "shopping-assistant",
    "data-processor"
  ]
}
```

## Troubleshooting

### "No components found"
- Ensure you're authenticated: `golem-cli --profile cloud cloud account get`
- Check deployment succeeded: Look for "Component deployed" message
- Verify in Golem Cloud console: https://release.golem.cloud

### "No workers found"
- Workers must be created after component deployment
- Use `golem-cli worker list` to verify
- Workers may take a few seconds to appear

### "No agent types found"
- Agent types may need to be registered through Golem Cloud console
- Check Golem documentation for agent registration
- Some Golem versions may not support agent types yet

### MCP tools still return empty
- Restart the MCP server after populating data
- Check you're using the correct profile: `--profile cloud`
- Verify the MCP server is using the cloud profile (not local)

## Alternative: Use Local Golem

If you have a local Golem instance running:

```bash
# Start local Golem
docker-compose up -d  # or however you run local Golem

# Deploy to local
cd rust-deploy
golem-cli app build
golem-cli app deploy  # defaults to local profile

# Create workers
golem-cli worker add --component deploy:counter --worker-name local-worker-1

# Test MCP with local profile
golem-cli mcp-server start --transport stdio
```

## Notes

- The MCP server uses the profile from your golem-cli configuration
- Default profile is `local` (localhost:9881)
- Use `--profile cloud` to target Golem Cloud
- Agent types functionality depends on your Golem version
