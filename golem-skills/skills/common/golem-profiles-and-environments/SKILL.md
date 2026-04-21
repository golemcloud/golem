---
name: golem-profiles-and-environments
description: "Understanding Golem CLI profiles, application environments, and component presets. Use when configuring deployment targets, switching between local and cloud servers, managing CLI profiles, defining environment-specific presets, or understanding how environments, presets, and profiles interact."
---

# Profiles, Environments, and Presets

Golem uses three related but distinct concepts for targeting deployments:

- **CLI Profiles** — stored in `~/.golem/config.json`, define how the CLI connects to a Golem server (URL, authentication)
- **App Environments** — defined in `golem.yaml` under `environments:`, describe deployment targets with server, account, presets, and CLI/deployment options
- **Component/Agent Presets** — defined in `golem.yaml` under `presets:` within components, agents, or component templates, provide named configuration overrides (env vars, plugins, files, config) that environments can activate

## How They Relate

```
CLI Profile (connection settings)
    ↑ resolved from
App Environment (deployment target)
    ↓ activates
Component/Agent Presets (config overrides)
```

When you run `golem deploy`, the CLI:

1. Selects an **environment** from the manifest (via `--local`, `--cloud`, `-e <name>`, or the `default: true` environment)
2. Resolves **connection settings** — either from the environment's `server:` field, or falls back to the active CLI **profile**
3. Activates **presets** listed in the environment's `componentPresets:` field, merging their overrides into the resolved configuration

## CLI Profiles

Profiles are global CLI configuration stored in `~/.golem/config.json`. They define server URLs and authentication credentials.

### Built-in profiles

| Profile | Server | Auth |
|---------|--------|------|
| `local` | `http://localhost:9881` | Built-in local token |
| `cloud` | `https://release.api.golem.cloud` | OAuth2 |

### Managing profiles

```shell
golem profile new                          # Interactive setup
golem profile new my-staging --url https://staging.example.com --static-token "..."
golem profile list                         # List all profiles
golem profile switch my-staging            # Set active profile
golem profile get                          # Show active profile
golem profile get my-staging               # Show specific profile
golem profile delete my-staging            # Delete a profile
golem profile config my-staging set-format json  # Set default output format
```

### Global flags

```shell
golem --profile my-staging deploy          # Use a specific profile for this command
golem -L deploy                            # Shortcut: use "local" environment/profile
golem -C deploy                            # Shortcut: use "cloud" environment/profile
```

### Profile fields

| Field | Description |
|-------|-------------|
| `custom_url` | Golem Component service URL |
| `custom_worker_url` | Golem Worker service URL (defaults to `custom_url`) |
| `allow_insecure` | Accept invalid TLS certificates |
| `auth` | Authentication — `staticToken` or OAuth2 |
| `config.default_format` | Default CLI output format (`text` or `json`) |

## App Environments

Environments are defined in `golem.yaml` and represent named deployment targets. They specify which server to deploy to, which presets to activate, and optional CLI/deployment behavior.

### Basic example

```yaml
environments:
  local:
    default: true                    # Selected when no -e flag is given
    server: local                    # Use built-in local server
    componentPresets: local          # Activate the "local" preset
  cloud:
    server: cloud                    # Use built-in Golem Cloud
    componentPresets: cloud
```

### Custom server example

```yaml
environments:
  staging:
    server:
      url: https://staging.example.com
      auth:
        staticToken: "{{ STAGING_TOKEN }}"
    componentPresets: staging
  prod:
    account: my-org                  # Cloud account to deploy to
    server: cloud
    componentPresets: prod
```

### Environment fields

| Field | Type | Description |
|-------|------|-------------|
| `default` | `true` | Mark as default environment (only one allowed) |
| `account` | string | Cloud account name for deployment |
| `server` | `local`, `cloud`, or custom object | Server connection settings |
| `componentPresets` | string or string[] | Preset name(s) to activate |
| `cli` | object | CLI behavior overrides (see below) |
| `deployment` | object | Deployment option overrides (see below) |

### Server options

- `local` — built-in local server (`http://localhost:9881`)
- `cloud` — Golem Cloud (`https://release.api.golem.cloud`) with OAuth2
- Custom object with `url`, optional `workerUrl`, optional `allowInsecure`, and `auth` (`oauth2: true` or `staticToken: "..."`)

### CLI options (`cli:`)

```yaml
environments:
  local:
    server: local
    cli:
      format: json                   # Default output format
      autoConfirm: true              # Auto-answer "yes" to prompts
      redeployAgents: true           # Equivalent to --reset on deploy
      reset: true                    # Reset all state on deploy
```

### Deployment options (`deployment:`)

```yaml
environments:
  local:
    server: local
    deployment:
      compatibilityCheck: false      # Skip component compatibility checks
      versionCheck: false            # Skip version mismatch checks
      securityOverrides: true        # Allow security config overrides
```

### Selecting an environment

| Flag | Effect |
|------|--------|
| `-L` / `--local` | Select the `local` environment (or `local` profile if no manifest) |
| `-C` / `--cloud` | Select the `cloud` environment (or `cloud` profile if no manifest) |
| `-e <name>` | Select a named environment from the manifest |
| *(none)* | Use the `default: true` environment, or fall back to active profile |

### Managing environments

```shell
golem environment list                     # List environments on the server
golem environment sync-deployment-options  # Sync deployment options to the server
```

### Environment-scoped manifest sections

Several top-level manifest sections are keyed by environment name:

```yaml
httpApi:
  deployments:
    local:                           # Only deployed to "local" environment
      - domain: localhost:9006
        agents:
          MyAgent: {}
    prod:
      - domain: api.example.com
        agents:
          MyAgent: {}

mcp:
  deployments:
    local:
      - domain: localhost:9006

secretDefaults:
  local:
    - path: ["agents", "MyAgent", "config", "api_key"]
      value: "test-key"
  prod:
    - path: ["agents", "MyAgent", "config", "api_key"]
      value: "{{ PROD_API_KEY }}"

retryPolicyDefaults:
  local:
    - name: default-retry
      priority: 10
      predicate: "true"
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential:
              baseDelay: { secs: 1, nanos: 0 }
              factor: 2.0

resourceDefaults:
  local:
    - name: api-calls
      limit: { type: Rate, value: 100, period: minute, max: 1000 }
      enforcementAction: reject
      unit: request
      units: requests
```

## Component and Agent Presets

Presets are named configuration overrides defined within components, agents, or component templates. They allow the same codebase to run with different settings per environment.

### Defining presets

Presets can be defined at the **component template**, **component**, or **agent** level:

```yaml
componentTemplates:
  shared-runtime:
    env:
      RUST_LOG: info
    presets:
      local:
        default: true                # Used when no preset is explicitly selected
        env:
          GOLEM_ENV: local
      cloud:
        env:
          GOLEM_ENV: cloud

components:
  my-app:service:
    templates: shared-runtime
    dir: service
    presets:
      local:
        build:
          - command: cargo build --target wasm32-wasip1
      release:
        build:
          - command: cargo build --target wasm32-wasip1 --release

agents:
  MyAgent:
    env:
      CACHE_TTL: "300"
    presets:
      local:
        env:
          CACHE_TTL: "60"
      cloud:
        env:
          CACHE_TTL: "3600"
```

### What presets can override

**Component presets** can override: `env`, `wasiConfig`, `plugins`, `files`, `config`, `build`, `componentWasm`, `outputWasm`, `customCommands`, `clean`.

**Agent presets** can override: `env`, `wasiConfig`, `plugins`, `files`, `config`.

Presets **cannot** override structural fields like `dir` or `templates`.

### How presets are activated

1. **Via environments**: The `componentPresets` field in an environment selects preset(s) by name:

   ```yaml
   environments:
     local:
       componentPresets: local       # Activates "local" preset everywhere
     staging:
       componentPresets: [staging, debug]  # Multiple presets (applied in order)
   ```

2. **Via CLI flag**: The `-P` / `--preset` flag overrides environment preset selection:

   ```shell
   golem build -P release
   golem deploy -P debug,verbose
   ```

3. **Default preset**: One preset per component/agent/template can be marked `default: true` — it is used when no preset is explicitly selected by environment or CLI flag.

> **Naming guideline**: Avoid using the same name for an environment and a preset (e.g. both called `local`). Although they are separate concepts, sharing names makes the manifest harder to read and reason about. Prefer distinct preset names such as `dev`, `debug`, `release`, or `optimized` that describe what the preset *does*, rather than mirroring the environment name.

### Cascade with presets

Presets are the most specific layer in the cascade:

```
componentTemplates → components → agents → presets
```

Preset properties merge with or override the base properties using the same merge modes (`envMergeMode`, `pluginsMergeMode`, etc.).

## Per-Agent Configuration

Configuration in the manifest is **per agent**. Each agent type can have its own:

- `env` — environment variables
- `files` — initial filesystem
- `config` — arbitrary typed configuration
- `plugins` — plugin installations

```yaml
agents:
  EscalationAgent:
    env:
      MODEL: claude-3-7-sonnet
      JIRA_TOKEN: "{{ JIRA_TOKEN }}"
    files:
      - sourcePath: ./prompts/escalation-system.md
        targetPath: /prompts/system.md
    config:
      projectKey: OPS
      defaultPriority: high

  AuditAgent:
    env:
      S3_BUCKET: supportdesk-audit
    config:
      retentionDays: 90
```

Each agent can also have its own presets, allowing environment-specific overrides per agent type.

## Common Patterns

### Local development + cloud production

```yaml
componentTemplates:
  shared:
    env:
      LOG_LEVEL: info
    presets:
      local:
        env:
          LOG_LEVEL: debug
      cloud:
        env:
          LOG_LEVEL: warn

environments:
  local:
    default: true
    server: local
    componentPresets: local
  prod:
    server: cloud
    componentPresets: cloud
```

### Multiple staging environments

```yaml
environments:
  local:
    default: true
    server: local
    componentPresets: local
  staging:
    server:
      url: https://staging.example.com
      auth:
        staticToken: "{{ STAGING_TOKEN }}"
    componentPresets: staging
  prod:
    account: my-org
    server: cloud
    componentPresets: prod
```

### Profile without a manifest

When no `golem.yaml` is found, the CLI falls back to profiles:

```shell
golem profile new my-server --url https://my-server.example.com --static-token "..."
golem --profile my-server component list   # Use profile directly
golem -L component list                    # Use built-in "local" profile
```

## Related Skills

- Load `golem-edit-manifest` for the complete manifest field reference
- Load `golem-deploy` for deployment commands and flags
- Load `golem-add-env-vars` for environment variable configuration details
- Load `golem-add-initial-files` for initial filesystem configuration
