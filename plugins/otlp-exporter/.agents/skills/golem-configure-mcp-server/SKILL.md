---
name: golem-configure-mcp-server
description: "Configuring MCP (Model Context Protocol) server deployments in golem.yaml. Use when the user asks to expose agents through MCP, enable MCP for an agent, add MCP server support, set up MCP authentication, or configure MCP deployments for local or cloud environments."
---

# Configuring MCP Server Deployments

## Overview

Any Golem agent can be automatically exposed as an **MCP (Model Context Protocol) server** without writing any extra code. MCP is enabled by adding an `mcp` section to `golem.yaml` — the same way HTTP API deployments are configured under `httpApi`.

The MCP server uses the **Streamable HTTP** transport. Once deployed, any MCP-compatible client (Claude Desktop, MCP Inspector, Cursor, custom clients) can connect to the server and interact with your agents through MCP tools, resources, and prompts.

## Adding an MCP Deployment

Add an `mcp` section to the root `golem.yaml`:

```yaml
mcp:
  deployments:
    local:
      - subdomain: my-app  # resolves to my-app.localhost:9007 by default
        agents:
          CounterAgent: {}
          TaskAgent: {}
```

### Structure

- `mcp.deployments` is a map keyed by **environment name** (e.g., `local`, `cloud`, `staging`)
- Each environment contains a list of deployment objects
- Each deployment has:
  - `subdomain`: a single DNS label (lowercase letters, digits, and hyphens only — no dots, port, or URL scheme) resolved through the target environment server. Local MCP deployments resolve to `<subdomain>.localhost:9007` by default, or `<subdomain>.localhost:<mcpPort>` when `localServer.mcpPort` is set to a stable nonzero port. Cloud MCP deployments resolve to `<subdomain>.mcps.golem.cloud`.
  - `domain`: a full domain such as `mcp.example.com` for custom registered domains or custom server environments.
  - `agents`: a map of agent type names (PascalCase) to their deployment options

Define exactly one of `subdomain` or `domain` on each deployment. Prefer `subdomain` for built-in `server: local` and `server: cloud` environments. Use `domain` when you need a full custom domain. `subdomain` cannot be used with custom server environments — those must use `domain`.

Do not use `localServer.mcpPort: 0` in the manifest. Port `0` is only allowed when passed directly as `--mcp-port 0` to `golem server run`; manifest deployment domains require stable nonzero ports.

### Local Development

For local development, the MCP server listens on port **9007** by default (separate from the HTTP API gateway on port 9006). Use `*.localhost:9007` domains:

```yaml
mcp:
  deployments:
    local:
      - subdomain: my-app  # resolves to my-app.localhost:9007 by default
        agents:
          MyAgent: {}
```

After deploying with `golem deploy --yes`, the MCP server is available at:

```
http://my-app.localhost:9007/mcp
```

### Cloud Deployment

For Golem Cloud's built-in server, configure the `cloud` environment and use `subdomain`:

```yaml
mcp:
  deployments:
    local:
      - subdomain: my-app  # resolves to my-app.localhost:9007 by default
        agents:
          MyAgent: {}
    cloud:
      - subdomain: my-app  # resolves to my-app.mcps.golem.cloud
        agents:
          MyAgent:
            securityScheme: my-oauth

environments:
  local:
    server: local
  cloud:
    server: cloud
```

Deploy to cloud with:

```shell
golem deploy --yes --cloud
```

For custom DNS or custom server environments, use `domain` with a full domain:

```yaml
mcp:
  deployments:
    prod:
      - domain: mcp.example.com
        agents:
          MyAgent:
            securityScheme: my-oauth
```

## Agent Options

Each agent entry accepts an optional `securityScheme` field:

```yaml
agents:
  PublicAgent: {}                          # No authentication
  SecureAgent:
    securityScheme: my-oidc                # Require OAuth authentication
```

## Security Schemes

MCP deployments support OAuth-based authentication through security schemes. Create a security scheme using the CLI:

### Creating a Security Scheme

```shell
golem api security-scheme create my-oidc \
  --provider-type google \
  --client-id "YOUR_CLIENT_ID" \
  --client-secret "YOUR_CLIENT_SECRET" \
  --redirect-url "http://my-app.localhost:9007/mcp/oauth/callback" \
  --scope openid --scope email --scope profile
```

For a custom OAuth provider:

```shell
golem api security-scheme create my-custom-oidc \
  --provider-type custom \
  --custom-provider-name "my-provider" \
  --custom-issuer-url "https://auth.example.com/realm" \
  --client-id "CLIENT_ID" \
  --client-secret "CLIENT_SECRET" \
  --redirect-url "http://my-app.localhost:9007/mcp/oauth/callback" \
  --scope openid --scope email --scope profile
```

### Supported Providers

| Provider | `--provider-type` value |
|---|---|
| Google | `google` |
| Facebook | `facebook` |
| Microsoft | `microsoft` |
| GitLab | `gitlab` |
| Custom OIDC | `custom` (requires `--custom-issuer-url`) |

### Referencing in golem.yaml

After creating a security scheme, reference it by name:

```yaml
mcp:
  deployments:
    local:
      - subdomain: my-app  # resolves to my-app.localhost:9007 by default
        agents:
          SecureAgent:
            securityScheme: my-oidc
```

**Important:** The OAuth callback URL in the security scheme must match the MCP server domain. The callback path is always `/mcp/oauth/callback`.

## Automatic MCP Mapping

Agent methods are automatically mapped to MCP entities based on these rules:

| Agent Type | Method | MCP Entity |
|---|---|---|
| Singleton | No parameters | Resource |
| Non-singleton | No parameters | Resource template |
| Any | Has parameters | Tool |

### Tool and Resource Naming

MCP tool and resource names are constructed as `{AgentTypeName}-{method_name}`, where the method name preserves the **source language naming convention** (not kebab-case):

| Language | Method in code | MCP tool name |
|---|---|---|
| Rust | `increment_by` | `CounterAgent-increment_by` |
| TypeScript | `incrementBy` | `CounterAgent-incrementBy` |
| Scala | `incrementBy` | `CounterAgent-incrementBy` |

Constructor parameters (which identify the agent instance) are automatically included in the tool's input schema alongside the method's own parameters.

### Agent and Method Metadata

Add `description` and `prompt` annotations to improve MCP discoverability:

**Rust:**
```rust
#[description("Increments the counter by n")]
#[prompt("Increment by a given number")]
fn increment_by(&mut self, n: u32) -> u32;
```

**TypeScript:**
```typescript
@description("Increments the counter by n")
@prompt("Increment by a given number")
async incrementBy(n: number): Promise<number> { ... }
```

**Scala:**
```scala
@description("Increments the counter by n")
@prompt("Increment by a given number")
def incrementBy(n: Int): Future[Int]
```

Both annotations are optional and are included in the MCP metadata sent to clients.

## Special Data Types for MCP

Golem supports special data types that map well to MCP concepts. These are not MCP-specific — agents using them can still be invoked through HTTP, RPC, etc.

### Unstructured Text

Accept free-form text input, optionally constrained by language:

**Rust:**
```rust
fn summarize(&self, text: UnstructuredText) -> String;
```

### Unstructured Binary

Accept binary data, optionally constrained by MIME type:

**Rust:**
```rust
#[derive(Debug, Clone, AllowedMimeTypes)]
enum Image {
    #[mime_type("image/png")] Png,
    #[mime_type("image/jpeg")] Jpeg,
}
fn process_image(&self, image: UnstructuredBinary<Image>) -> String;
```

### Multimodal

Accept mixed text, binary, or structured data:

**Rust:**
```rust
fn analyze(&self, input: Multimodal) -> String;
```

## Multi-Environment Deployments

Configure different domains and security settings per environment:

```yaml
mcp:
  deployments:
    local:
      - subdomain: my-app  # resolves to my-app.localhost:9007 by default
        agents:
          MyAgent: {}
    cloud:
      - subdomain: my-app  # resolves to my-app.mcps.golem.cloud
        agents:
          MyAgent:
            securityScheme: prod-google-oidc

environments:
  local:
    server: local
  cloud:
    server: cloud
```

## Deploying

After configuring `golem.yaml`, deploy. Always use `--yes` to avoid interactive prompts:

```shell
golem deploy --yes               # Deploy to the default environment (usually local)
golem deploy --yes --cloud       # Deploy to the cloud environment
```

## Testing with MCP Inspector

You can verify your MCP server using the [MCP Inspector](https://modelcontextprotocol.io/docs/tools/inspector):

```shell
npx @modelcontextprotocol/inspector
```

Then connect to `http://my-app.localhost:9007/mcp` using the Streamable HTTP transport.

## Complete Example

```yaml
# golem.yaml

mcp:
  deployments:
    local:
      - subdomain: counter-app  # resolves to counter-app.localhost:9007 by default
        agents:
          CounterAgent: {}
    cloud:
      - subdomain: counter-app  # resolves to counter-app.mcps.golem.cloud
        agents:
          CounterAgent:
            securityScheme: google-oidc

environments:
  local:
    server: local
  cloud:
    server: cloud
```

## Key Constraints

- Agent type names in `golem.yaml` use **PascalCase** (matching the class/trait name in code)
- The MCP server listens on port **9007** by default for local development (separate from the HTTP API gateway port 9006)
- The MCP endpoint path is always `/mcp` (e.g., `http://my-app.localhost:9007/mcp`)
- Security schemes must be created via `golem api security-scheme create` before they can be referenced
- The resolved domain must be unique per environment
- After changing `golem.yaml`, run `golem deploy --yes` to apply changes
- The OAuth callback path for MCP security schemes is `/mcp/oauth/callback`
