---
name: golem-edit-manifest
description: "Editing the Golem Application Manifest (golem.yaml). Use when modifying any section of golem.yaml — components, agents, templates, presets, environments, httpApi, mcp, bridge SDKs, build commands, config, wasiConfig, plugins, files, custom commands, clean paths, retry policies, resource quotas, or secret defaults."
---

# Editing the Golem Application Manifest (golem.yaml)

The `golem.yaml` file in the project root is the **Golem Application Manifest**. It defines the entire application: components, agents, build steps, environments, HTTP/MCP deployments, and more.

**Schema version**: The `manifestVersion` field at the top of the file identifies the schema version. Do not change it.

## Top-Level Structure

```yaml
manifestVersion: "1.5.0-dev.3"   # Schema version — do not change
app: my-app                       # Application name

includes:                          # (optional) Glob patterns for additional manifest fragments
  - "components/*/golem.yaml"

componentTemplates:                # Reusable property layers (build, env, plugins, files)
  <template-name>: { ... }

components:                        # Component definitions by name (namespace:name)
  <ns:name>: { ... }

agents:                            # Agent type definitions by PascalCase name
  <AgentName>: { ... }

environments:                      # Deployment environments (local, cloud, custom)
  <env-name>: { ... }

httpApi:                           # HTTP API domain deployments
  deployments: { ... }

mcp:                               # MCP (Model Context Protocol) deployments
  deployments: { ... }

bridge:                            # Bridge SDK generation configuration
  ts: { ... }
  rust: { ... }

customCommands:                    # User-defined CLI commands
  <command-name>: [...]

clean:                             # Extra paths for `golem clean`
  - "path/to/clean"

secretDefaults:                    # Secret defaults per environment
  <env-name>: {...}

retryPolicyDefaults:               # Retry policy defaults per environment
  <env-name>: {...}

resourceDefaults:                  # Quota resource defaults per environment
  <env-name>: {...}
```

## Components

Components are keyed by `namespace:name` (e.g., `my-app:billing`).

```yaml
components:
  my-app:billing:
    dir: billing                   # Base directory (relative to golem.yaml). Use "." for single-component apps
    templates:                     # Parent template names (inherit build, env, plugins, files)
      - rust
    componentWasm: target/wasm32-wasip1/debug/billing.wasm   # Path to built WASM
    outputWasm: golem-temp/billing.wasm                       # Path to final output WASM
    build:                         # Build commands (see Build Commands below)
      - command: cargo build --target wasm32-wasip1
    env:                           # Environment variables
      LOG_LEVEL: info
    plugins:                       # Plugin installations
      - name: otlp-exporter
        version: "0.1.0"
    files:                         # Initial filesystem entries
      - sourcePath: ./data/config.json
        targetPath: /etc/config.json
    clean:                         # Extra clean paths
      - target/
    presets:                       # Named presets with overrides
      debug:
        default: true
        env:
          LOG_LEVEL: debug
      release:
        env:
          LOG_LEVEL: warn
```

### Key component fields

| Field | Type | Description |
|-------|------|-------------|
| `dir` | string | Base directory for resolving paths. `"."` for single-component apps |
| `templates` | string or string[] | Parent template name(s) to inherit from |
| `componentWasm` | string | Path to the built WASM component |
| `outputWasm` | string | Path for the final output WASM ready for upload |
| `build` | array | Build commands (see Build Commands) |
| `env` | map | Environment variables (string → string) |
| `envMergeMode` | enum | `upsert` (default), `replace`, or `remove` |
| `plugins` | array | Plugin installations |
| `pluginsMergeMode` | enum | `append` (default), `prepend`, or `replace` |
| `files` | array | Initial filesystem entries |
| `filesMergeMode` | enum | `append` (default), `prepend`, or `replace` |
| `config` | any | Arbitrary configuration passed to agent |
| `wasiConfig` | map | WASI configuration variables (string → string) |
| `wasiConfigMergeMode` | enum | `upsert` (default), `replace`, or `remove` |
| `customCommands` | map | Component-level custom commands |
| `clean` | string[] | Extra clean paths |
| `presets` | map | Named presets (see Presets) |

## Component Templates

Templates define reusable property layers. Components reference them via `templates:`.

```yaml
componentTemplates:
  rust:
    build:
      - command: cargo build --target wasm32-wasip1
    env:
      RUST_LOG: info

components:
  my-app:service:
    templates: [rust]              # Inherits build and env from "rust" template
    dir: service
```

Templates support the same fields as components except `dir`. Templates can themselves reference other templates via `templates:`.

## Agents

Agent types are keyed by PascalCase name. They support the same cascading property fields as components (env, plugins, files, config, wasiConfig) but NOT build-related fields. Agents can also inherit from templates via `templates:`.

```yaml
agents:
  MyAgent:
    templates: [shared-runtime]    # Inherit env/plugins/files from a template
    env:
      CACHE_TTL: "300"
    plugins:
      - name: otlp-exporter
        version: "0.1.0"
    files:
      - sourcePath: ./agent-data/model.bin
        targetPath: /data/model.bin
    presets:
      debug:
        default: true
        env:
          CACHE_TTL: "60"
```

## Config and WASI Config

The `config` field accepts arbitrary YAML values (objects, arrays, scalars) passed as typed configuration to the agent at runtime. The `wasiConfig` field is a string-to-string map of WASI-level configuration variables.

```yaml
components:
  my-app:service:
    config:
      model: gpt-4o-mini
      temperature: 0.2
      features:
        - summarize
        - translate
    wasiConfig:
      WASI_FLAG: enabled

agents:
  MyAgent:
    config:
      max_retries: 3
    wasiConfig:
      AGENT_WASI_OPT: "true"
```

Both fields follow the cascade hierarchy and support merge modes (`wasiConfigMergeMode` for `wasiConfig`).

## Cascade / Merge System

Properties cascade from general to specific:

```
componentTemplates → components → agents → presets
```

Each level can override or merge with its parent using merge modes:

| Property | Merge Mode Field | Type | Default | Options |
|----------|-----------------|------|---------|---------|
| `env` | `envMergeMode` | map | `upsert` | `upsert`, `replace`, `remove` |
| `wasiConfig` | `wasiConfigMergeMode` | map | `upsert` | `upsert`, `replace`, `remove` |
| `plugins` | `pluginsMergeMode` | vec | `append` | `append`, `prepend`, `replace` |
| `files` | `filesMergeMode` | vec | `append` | `append`, `prepend`, `replace` |
| `build` | `buildMergeMode` | vec | `append` | `append`, `prepend`, `replace` |

## Presets

Both components and agents support `presets` — named configurations that can override the properties allowed at that level. Component presets can override component-layer fields (build, env, plugins, files, config, wasiConfig, clean, customCommands); agent presets can override agent-layer fields (env, plugins, files, config, wasiConfig). Presets cannot override structural fields like `dir` or `templates`. Mark one preset as default with `default: true`.

```yaml
components:
  my-app:service:
    presets:
      debug:
        default: true               # Selected by default
        env:
          LOG_LEVEL: debug
      release:
        env:
          LOG_LEVEL: warn
```

## Build Commands

The `build` array contains commands executed during `golem build`. Each entry is one of:

### External command (most common)

```yaml
build:
  - command: cargo build --target wasm32-wasip1
    dir: .                         # Optional working directory
    env:                           # Optional extra env vars
      RUSTFLAGS: "-C opt-level=2"
    rmdirs: [target/old]           # Directories to delete before running (runs before mkdirs)
    mkdirs: [target/new]           # Directories to create before running (runs after rmdirs)
    sources: ["src/**/*.rs"]       # Inputs for up-to-date checks
    targets: ["target/wasm32-wasip1/debug/*.wasm"]  # Outputs for up-to-date checks
```

### TypeScript/QuickJS-specific commands

```yaml
build:
  - generateQuickjsCrate: golem-temp/quickjs-crate   # Generate QuickJS Rust crate
    wit: wit
    jsModules: { "main.js": "esm" }
    world: my-world                                    # Optional WIT world

  - generateQuickjsDts: golem-temp/bindings.d.ts      # Generate TypeScript declarations
    wit: wit

  - injectToPrebuiltQuickjs: golem-temp/quickjs.wasm  # Inject JS into prebuilt QuickJS
    module: dist/bundle.js
    into: golem-temp/output.wasm

  - preinitializeJs: golem-temp/output.wasm            # Pre-initialize JS runtime
    into: golem-temp/preinit.wasm
```

## Custom Commands

Define CLI commands at the application or component level:

```yaml
customCommands:
  test:
    - command: cargo test --target wasm32-wasip1
      dir: .
  lint:
    - command: cargo clippy --target wasm32-wasip1
```

Run with `golem exec <name>` (e.g., `golem exec test`).

`customCommands` can only contain `command:`-style external commands; they do not support `generateQuickjsCrate`, `injectToPrebuiltQuickjs`, or other build-specific command types.

## Environments

Environments configure where and how the application is deployed.

```yaml
environments:
  local:
    default: true                  # First environment is default if not specified
    server: local                  # Built-in local server
    cli:
      format: text
      autoConfirm: true
  cloud:
    server: cloud                  # Golem Cloud
  staging:
    server:                        # Custom server
      url: https://staging.example.com
      auth:
        oauth2: true               # or: staticToken: "my-token"
      workerUrl: https://staging-workers.example.com   # Optional separate worker URL
      allowInsecure: false         # Optional, allow insecure connections
    componentPresets: [release]    # Preset names to activate
    cli:
      format: json
      redeployAgents: true
      reset: true
    deployment:
      compatibilityCheck: true
      versionCheck: true
      securityOverrides: false
```

### Server options

| Value | Description |
|-------|-------------|
| `local` | Built-in local Golem server |
| `cloud` | Golem Cloud |
| `{ url, auth, ... }` | Custom server (see Custom Server below) |

### Custom server auth

```yaml
# OAuth2 (interactive login)
auth:
  oauth2: true

# Static token
auth:
  staticToken: "my-secret-token"
```

### CLI options

| Field | Description |
|-------|-------------|
| `format` | Default output: `text`, `json`, `yaml`, `pretty`, `pretty-json`, `pretty-yaml` |
| `autoConfirm` | Auto-confirm prompts (`true`) |
| `redeployAgents` | Redeploy agents by default (`true`) |
| `reset` | Reset agents by default (`true`) |

### Deployment options

| Field | Type | Description |
|-------|------|-------------|
| `compatibilityCheck` | bool | Check component compatibility before deploying |
| `versionCheck` | bool | Check version constraints |
| `securityOverrides` | bool | Allow security scheme overrides |

## HTTP API Deployments

Configure HTTP API domain deployments per environment.

```yaml
httpApi:
  deployments:
    local:
      - domain: my-app.localhost:9006
        webhookUrl: http://my-app.localhost:9006   # Optional webhook base URL
        agents:
          TaskAgent: {}                             # No auth
          SecureAgent:
            securityScheme: my-oidc                 # OIDC security scheme name
          DevAgent:
            testSessionHeaderName: X-Test-Auth      # Test auth header
    prod:
      - domain: api.myapp.com
        agents:
          TaskAgent: {}
          SecureAgent:
            securityScheme: prod-google-oidc
```

Agent names use **PascalCase** matching the agent type name in code.

## MCP Deployments

Configure MCP (Model Context Protocol) deployments per environment.

```yaml
mcp:
  deployments:
    local:
      - domain: mcp.localhost:9006
        agents:
          ToolAgent: {}
          SecureToolAgent:
            securityScheme: my-oidc
```

## Bridge SDK Generation

Generate typed client SDKs for calling agents from external code. The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (namespace:name).

```yaml
bridge:
  ts:
    agents: "*"                    # Generate for all agents
    outputDir: ./bridge-sdk/ts     # Optional custom output directory
  rust:
    agents:
      - MyAgent                    # Agent type name
      - my-app:billing             # Component name (all agents in that component)
    outputDir: ./bridge-sdk/rust
```

## Plugin Installations

Plugins are installed at any cascade level (template, component, agent, preset).

```yaml
plugins:
  - name: otlp-exporter
    version: "0.1.0"
    account: golem                 # Optional account
    parameters:                    # Optional key-value config
      endpoint: http://localhost:4317
      protocol: grpc
```

## Initial Files

Mount files into the agent's virtual filesystem.

```yaml
files:
  - sourcePath: ./data/config.json       # Local path or URL
    targetPath: /etc/app/config.json      # Absolute path in agent filesystem
    permissions: read-only                # read-only (default) or read-write
  - sourcePath: ./static-assets/          # Directory — recursively included
    targetPath: /var/www/static/
  - sourcePath: https://example.com/model.bin   # Remote URL
    targetPath: /data/model.bin
```

## Template Substitution

Environment variable values support `{{ VAR_NAME }}` syntax. At deploy time, these resolve against the host machine's environment:

```yaml
env:
  API_KEY: "{{ MY_API_KEY }}"
  DB_URL: "prefix-{{ DB_HOST }}-suffix"
```

Missing host variables cause deployment failure.

## Secret Defaults

Secret defaults per environment use the same nested object style as `config`:

```yaml
secretDefaults:
  local:
    apiKey: "test-key-123"
  prod:
    apiKey: "{{ PROD_API_KEY }}"
```

## Retry Policy Defaults

Named retry policies created in the environment during deployment:

```yaml
retryPolicyDefaults:
  local:
    default-retry:
      priority: 10
      predicate: "true"                    # Always match
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential:
              baseDelay: { secs: 1, nanos: 0 }
              factor: 2.0
```

### Retry policy types

| Type | Fields | Description |
|------|--------|-------------|
| `"immediate"` | — | Retry immediately |
| `"never"` | — | Never retry |
| `periodic` | `{ secs, nanos }` | Fixed delay between retries |
| `exponential` | `{ baseDelay, factor }` | Exponentially increasing delay |
| `fibonacci` | `{ first, second }` | Fibonacci-sequence delays |
| `countBox` | `{ maxRetries, inner }` | Limit total retry count |
| `timeBox` | `{ limit, inner }` | Limit total retry time |
| `clamp` | `{ minDelay, maxDelay, inner }` | Clamp delay range |
| `addDelay` | `{ delay, inner }` | Add fixed delay to inner policy |
| `jitter` | `{ factor, inner }` | Add random jitter |
| `filteredOn` | `{ predicate, inner }` | Apply only when predicate matches |
| `andThen` | `[policy1, policy2]` | Sequential composition |
| `union` | `[policy1, policy2]` | Union of two policies |
| `intersect` | `[policy1, policy2]` | Intersection of two policies |

### Retry predicates

| Type | Fields | Description |
|------|--------|-------------|
| `"true"` | — | Always matches |
| `"false"` | — | Never matches |
| `propEq` | `{ property, value }` | Property equals value |
| `propNeq` | `{ property, value }` | Property not equal |
| `propGt` / `propGte` | `{ property, value }` | Greater than / greater or equal |
| `propLt` / `propLte` | `{ property, value }` | Less than / less or equal |
| `propExists` | string | Property exists |
| `propIn` | `{ property, values }` | Property in set |
| `propMatches` | `{ property, pattern }` | Regex match |
| `propStartsWith` | `{ property, prefix }` | Starts with prefix |
| `propContains` | `{ property, substring }` | Contains substring |
| `and` | `[pred1, pred2]` | Logical AND |
| `or` | `[pred1, pred2]` | Logical OR |
| `not` | predicate | Logical NOT |

Predicate values are typed: `{ text: "..." }`, `{ integer: 42 }`, or `{ boolean: true }`.

## Resource Quota Defaults

Quota resource definitions created during deployment:

```yaml
resourceDefaults:
  local:
    api-calls:
      limit:
        type: Rate
        value: 100
        period: minute
        max: 1000
      enforcementAction: reject        # reject, throttle, or terminate
      unit: request
      units: requests
    storage:
      limit:
        type: Capacity
        value: 1073741824              # 1 GB
      enforcementAction: reject
      unit: byte
      units: bytes
    - name: connections
      limit:
        type: Concurrency
        value: 50
      enforcementAction: throttle
      unit: connection
      units: connections
```

### Resource limit types

| Type | Required Fields | Description |
|------|----------------|-------------|
| `Rate` | `value`, `period`, `max` | Rate limit per time period. `period`: second/minute/hour/day/month/year |
| `Capacity` | `value` | Total capacity limit |
| `Concurrency` | `value` | Concurrent usage limit |

### Enforcement actions

| Action | Description |
|--------|-------------|
| `reject` | Reject requests exceeding the limit |
| `throttle` | Slow down requests exceeding the limit |
| `terminate` | Terminate the agent when limit is exceeded |

## Common Edit Patterns

### Add a new component

Insert a new key under `components:`:

```yaml
components:
  my-app:new-service:
    dir: new-service
    templates: [rust]
```

### Add environment variables to an agent

```yaml
agents:
  MyAgent:
    env:
      NEW_VAR: "value"
```

### Add a plugin to a component

```yaml
components:
  my-app:service:
    plugins:
      - name: my-plugin
        version: "1.0.0"
        parameters:
          key: value
```

### Add a new environment

```yaml
environments:
  staging:
    server:
      url: https://staging.example.com
      auth:
        staticToken: "{{ STAGING_TOKEN }}"
```

### Add HTTP API deployment for a new environment

```yaml
httpApi:
  deployments:
    staging:
      - domain: api-staging.example.com
        agents:
          MyAgent: {}
```

### Add clean paths

Root-level clean paths apply to `golem clean` globally; component-level clean paths are scoped to that component:

```yaml
clean:
  - golem-temp/
  - dist/

components:
  my-app:web:
    clean:
      - node_modules/.cache/
      - build/
```

## Field Scope Matrix

This table shows where each property can be defined:

| Field | Root | Component Template | Component | Agent | Component Preset | Agent Preset |
|-------|:----:|:------------------:|:---------:|:-----:|:----------------:|:------------:|
| `templates` | — | ✅ | ✅ | ✅ | — | — |
| `build` | — | ✅ | ✅ | — | ✅ | — |
| `env` | — | ✅ | ✅ | ✅ | ✅ | ✅ |
| `wasiConfig` | — | ✅ | ✅ | ✅ | ✅ | ✅ |
| `plugins` | — | ✅ | ✅ | ✅ | ✅ | ✅ |
| `files` | — | ✅ | ✅ | ✅ | ✅ | ✅ |
| `config` | — | ✅ | ✅ | ✅ | ✅ | ✅ |
| `customCommands` | ✅ | ✅ | ✅ | — | ✅ | — |
| `clean` | ✅ | ✅ | ✅ | — | ✅ | — |
| `dir` | — | — | ✅ | — | — | — |
| `componentWasm` | — | ✅ | ✅ | — | ✅ | — |
| `outputWasm` | — | ✅ | ✅ | — | ✅ | — |

## Related Skills

- Load `golem-profiles-and-environments` for detailed guidance on CLI profiles, app environments, component presets, and how they interact

## Edit Guardrails

- **Do not invent fields**: most manifest objects use `additionalProperties: false` — only use fields documented above.
- **Preserve `manifestVersion`**: never change the schema version.
- **Agent names use PascalCase**: matching the class/trait name in code (e.g., `MyAgent`, not `my-agent`).
- **Component names use `namespace:name`** format (e.g., `my-app:billing`).
- **Only one `default: true` preset** per preset map.
- **Merge modes are intentional**: `env`, `wasiConfig`, `plugins`, `files`, and `build` respect merge modes. Don't silently replace arrays/maps unless a merge mode of `replace` is set.
- **Template substitution** (`{{ VAR }}`) in env values resolves from host environment at deploy time. Missing variables cause deployment failure.
