---
name: golem-configure-api-domain
description: "Configuring HTTP API domain deployments and security schemes in golem.yaml. Use when the user asks to deploy agents to a domain, configure API domains, set up authentication/security schemes (OIDC), or manage the httpApi section of the application manifest."
---

# Configuring HTTP API Domain Deployments

## Overview

After adding HTTP mounts and endpoints to agents in code, you must configure a **domain deployment** in `golem.yaml` so Golem knows which agents to expose and on which domain. This skill covers the `httpApi` manifest section, security scheme setup, and the auto-generated OpenAPI specification.

## Adding a Domain Deployment

Add an `httpApi` section to the root `golem.yaml`:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        TaskAgent: {}
        UserAgent: {}
```

### Structure

- `httpApi.deployments` is a map keyed by **environment name** (e.g., `local`, `staging`, `prod`)
- Each environment contains a list of deployment objects
- Each deployment has:
  - `domain`: the (sub)domain to bind to (e.g., `my-app.localhost:9006` for local development)
  - `agents`: a map of agent type names (PascalCase) to their deployment options
  - `webhookUrl` (optional): base URL for webhook callbacks

### Agent Options

Each agent entry accepts these optional fields:

```yaml
agents:
  TaskAgent: {}                          # Default — no auth, no test header
  SecureAgent:
    securityScheme: my-oidc              # Reference a security scheme by name
  DevAgent:
    testSessionHeaderName: X-Test-Auth   # Use a test header for development
```

- `securityScheme`: name of a pre-configured security scheme (see below) — use this when the agent has `auth: true`
- `testSessionHeaderName`: header name for test/development authentication — provides a simple way to pass identity without a real OIDC flow
- Only one of `securityScheme` or `testSessionHeaderName` can be set per agent

## Security Schemes

Security schemes define OIDC authentication providers. They are managed via the Golem CLI:

### Creating a Security Scheme

```shell
golem api security-scheme create my-oidc \
  --provider-type google \
  --client-id "YOUR_CLIENT_ID" \
  --client-secret "YOUR_CLIENT_SECRET" \
  --redirect-url "http://localhost:9006/auth/callback" \
  --scope openid --scope email --scope profile
```

### Supported Providers

| Provider | `--provider-type` value |
|---|---|
| Google | `google` |
| Facebook | `facebook` |
| Microsoft | `microsoft` |
| GitLab | `gitlab` |
| Custom OIDC | `custom` (requires `--issuer-url`) |

For a custom OIDC provider:

```shell
golem api security-scheme create my-custom-oidc \
  --provider-type custom \
  --issuer-url "https://auth.example.com" \
  --client-id "YOUR_CLIENT_ID" \
  --client-secret "YOUR_CLIENT_SECRET" \
  --redirect-url "https://app.example.com/auth/callback" \
  --scope openid
```

### Managing Security Schemes

```shell
golem api security-scheme get my-oidc           # View details
golem api security-scheme update my-oidc ...     # Update fields
golem api security-scheme delete my-oidc         # Delete
```

### Referencing in golem.yaml

After creating a security scheme, reference it by name in the agent deployment options:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        SecureAgent:
          securityScheme: my-oidc
```

This enables OIDC authentication for all endpoints on `SecureAgent` that have `auth: true` set in their code-level annotations.

### Test Session Header (Development)

For local development without a real OIDC provider, use a test session header:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        SecureAgent:
          testSessionHeaderName: X-Test-Auth
```

Then pass identity in requests. The header value must be a **JSON object** representing an OIDC session. All fields have defaults, so only `subject` is needed to identify the caller:

```shell
curl -H 'X-Test-Auth: {"subject":"test-user-id"}' http://my-app.localhost:9006/secure/agent1/data
```

Available fields (all optional, with defaults):

| Field | Type | Default |
|---|---|---|
| `subject` | string | `"test-user"` |
| `issuer` | string (URL) | `"http://test-idp.com"` |
| `email` | string | `null` |
| `name` | string | `null` |
| `email_verified` | boolean | `null` |
| `given_name` | string | `null` |
| `family_name` | string | `null` |
| `picture` | string (URL) | `null` |
| `preferred_username` | string | `null` |
| `scopes` | array of strings | `["openid"]` |
| `issued_at` | ISO 8601 datetime | current time |
| `expires_at` | ISO 8601 datetime | current time + 8 hours |

> **⚠️ Important:** The header value must be valid JSON — a plain string like `"user1"` will be rejected with a 400 error.

## Multi-Environment Deployments

Define different domains and security configurations per environment:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        TaskAgent: {}
        SecureAgent:
          testSessionHeaderName: X-Test-Auth
    prod:
    - domain: api.myapp.com
      agents:
        TaskAgent: {}
        SecureAgent:
          securityScheme: prod-google-oidc
```

## Webhook URL

If agents use webhooks, configure the base URL:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      webhookUrl: http://my-app.localhost:9006
      agents:
        WebhookAgent: {}
```

The `webhookUrl` is combined with the agent's `webhookSuffix` (defined in code) to form the full webhook callback URL.

## Deploying

After configuring `golem.yaml`, deploy. Always use `--yes` to avoid interactive prompts:

```shell
golem deploy --yes                     # Deploy all components and HTTP API
golem deploy --yes --reset             # Deploy and delete all previously created agents
golem deploy --yes --try-update-agents # Deploy and update running agents
```

## Auto-Generated OpenAPI

Golem automatically serves an OpenAPI specification at `/openapi.yaml` on each deployment domain:

```shell
curl http://my-app.localhost:9006/openapi.yaml
```

This specification includes all endpoints from all agents deployed to that domain, with proper path parameters, request/response schemas, and CORS metadata.

## Complete Example

```yaml
# golem.yaml

httpApi:
  deployments:
    local:
    - domain: task-app.localhost:9006
      webhookUrl: http://task-app.localhost:9006
      agents:
        TaskAgent: {}
        AdminAgent:
          testSessionHeaderName: X-Admin-Auth
    prod:
    - domain: api.taskapp.com
      agents:
        TaskAgent: {}
        AdminAgent:
          securityScheme: google-oidc
```

## Key Constraints

- Agent type names in `golem.yaml` use **PascalCase** (matching the class/trait name in code)
- Each agent entry can have at most one of `securityScheme` or `testSessionHeaderName`
- Security schemes must be created via `golem api security-scheme create` before they can be referenced
- The domain must be unique per environment
- After changing `golem.yaml`, run `golem deploy --yes` to apply changes
