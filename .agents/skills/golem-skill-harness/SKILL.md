---
name: golem-skill-harness
description: "Developing, testing, and running Golem skill tests with the skill test harness. Use when creating new skills, writing scenario YAML files, running skill tests locally, or debugging skill test failures."
---

# Golem Skill Test Harness

The skill test harness lives in `golem-skills/tests/harness/`. It drives coding agents (Claude Code, OpenCode, Codex) through scenario YAML files, verifying that skills are activated and produce correct results. Skill definitions live in `golem-skills/skills/`.

## Prerequisites

- **Node.js 20+** and npm
- **Golem binary** pre-built: the harness requires a `golem` binary in `$GOLEM_PATH/target/release/` or `$GOLEM_PATH/target/debug/`. Build with `cargo build -p golem` (debug) or `cargo build -p golem --release` (release). The harness prefers the release build and falls back to debug.
- **No pre-running Golem server**: the harness starts its own server automatically using `golem server run --data-dir <workspaces/golem-server-data> --clean` and stops it when done. If a server is already running on port 9881, the harness **fails with an error** to avoid conflicts.
- **Agent CLI** installed: one of `claude` (Claude Code), `opencode`, or `codex`
- **Filesystem watcher**: `fswatch` on macOS, `inotify-tools` on Linux
- **GOLEM_PATH** env var set to the golem repo root. If not set, the harness auto-detects it by walking up from `cwd` looking for `sdks/rust/golem-rust` and `sdks/ts/packages` directories (same markers as `golem-cli`). If auto-detection also fails, the harness exits with an error. The resolved target directory (`target/release` or `target/debug`) is prepended to `PATH` so all spawned processes — including agent drivers — use the correct `golem` and `golem-cli` binaries.
- For Rust skills: `cargo-component` and `wasm32-wasip2` target
- For TS skills: `pnpm`, `wasm-rquickjs-cli`, TS SDK built (`cargo make build-sdk-ts`)

## Install and Build

```shell
cd golem-skills/tests/harness
npm install
npm run build
```

## Running Unit Tests (harness self-tests)

```shell
cd golem-skills/tests/harness
npm test
```

## Running Skill Scenarios

From `golem-skills/tests/harness/`:

```shell
npx tsx src/run.ts [options]
```

### CLI Options

| Option | Description | Default |
|--------|-------------|---------|
| `--agent <name>` | Agent driver: `claude-code`, `opencode`, `codex`, or `all` | `all` |
| `--language <lang>` | Language: `ts`, `rust`, or `all` | `all` |
| `--scenario <name>` | Run only the named scenario | all scenarios |
| `--scenarios <dir>` | Path to scenario YAML directory | `./scenarios` |
| `--output <dir>` | Results output directory | `./results` |
| `--timeout <seconds>` | Global timeout per step | `300` |
| `--skills <dir>` | Path to skills directory | `../../skills` |
| `--dry-run` | Validate scenarios without executing | `false` |
| `--resume-from <id>` | Resume from a specific step ID | — |
| `--workspace <path>` | Override workspace directory | — |
| `--merge-reports <dir>` | Merge summary.json files into aggregated report | — |

### Examples

```shell
# Run a single scenario with Claude Code for Rust
npx tsx src/run.ts --agent claude-code --language rust --scenario golem-new-project-rust

# Dry-run to validate YAML
npx tsx src/run.ts --dry-run --scenario golem-db-app-ts

# Resume a failed scenario from a specific step, reusing a previous workspace
npx tsx src/run.ts --agent claude-code --language ts --scenario golem-db-app-ts \
  --resume-from build-and-deploy --workspace ./workspaces/<run-id>/golem-db-app-ts/ts

# Merge reports from multiple CI runs
npx tsx src/run.ts --merge-reports ./ci-results --output ./merged
```

### Workspace Directory

Each harness run generates a unique run ID (UUID). Without `--workspace`, each scenario gets its own directory at `<cwd>/workspaces/<run-id>/<scenario-name>/<language>/`. Workspace directories are never deleted, so you can inspect the results after the run. With `--workspace`, the specified directory is used directly.

### Golem Server Lifecycle

The harness manages the Golem server automatically:
1. **Startup**: Before running scenarios, the harness checks port 9881. If a server is already running, it **fails with an error**. Otherwise it starts `golem server run --data-dir <workspaces/<run-id>/golem-server-data> --clean` and waits up to 60 seconds for the healthcheck to pass.
2. **Between scenarios**: The server is restarted (stopped and started again with `--clean`) to ensure a fresh state for each scenario.
3. **Per-scenario check**: Before each scenario, the harness verifies that a `local` Golem profile exists and the server is still reachable.
4. **Teardown**: After all scenarios complete (or on Ctrl+C), the harness stops the server process.

## Adding a New Skill

### 1. Create the skill definition

Create `golem-skills/skills/<skill-name>/SKILL.md` with YAML frontmatter:

```markdown
---
name: my-new-skill
description: "What the skill does. Use when <trigger conditions>."
---

# Skill Title

Instructions for the agent...
```

### 2. Write a scenario YAML

Create `golem-skills/tests/harness/scenarios/<scenario-name>.yaml`:

```yaml
name: "my-scenario"
settings:
  timeout_per_subprompt: 300
  golem_server:
    custom_request_port: 9006
steps:
  - id: "step-one"
    prompt: "Do something using the skill"
    expectedSkills:
      - "my-new-skill"
    verify:
      build: true
```

### 3. Run the scenario

```shell
npx tsx src/run.ts --agent claude-code --language rust --scenario my-scenario --skills ../../skills
```

## Scenario YAML Reference

### Top-Level Fields

```yaml
name: "scenario-name"              # Required. Unique scenario identifier.
settings:
  timeout_per_subprompt: 300       # Default timeout for prompt steps (seconds)
  golem_server:
    router_port: 9881              # Golem router port (for healthcheck)
    custom_request_port: 9006      # Sets GOLEM_CUSTOM_REQUEST_PORT env var
  cleanup: true                    # Whether to clean workspace before run
prerequisites:
  env:                             # Extra env vars set during execution
    DATABASE_URL: "postgres://..."
skip_if:                           # Skip entire scenario conditionally
  language: "ts"                   # Skip when language is "ts"
  agent: "codex"                   # Skip when agent is "codex"
  os: "windows"                    # Skip when OS matches (darwin→macos, win32→windows)
steps: [...]                       # Required. At least one step.
```

### Step Types

Every step must have **exactly one** action field. Common fields available on all steps:

```yaml
- id: "unique-step-id"             # Optional. Used for --resume-from.
  timeout: 600                     # Override step timeout (seconds)
  expect: { ... }                  # Assertions (see below)
  retry:                           # Retry on failure
    attempts: 3
    delay: 5                       # Seconds between retries
  only_if:                         # Run only when conditions match
    language: "rust"
    agent: "claude-code"
    os: "macos"
  skip_if:                         # Skip when conditions match
    language: "ts"
```

#### `prompt` — Send a prompt to the coding agent

```yaml
- id: "create-app"
  prompt: "Create a new Golem application called my-app with Rust."
  expectedSkills:                  # Skills that MUST be activated
    - "golem-new-project"
  allowedExtraSkills:              # Extra skills that are OK to activate
    - "golem-db-app-rust"
  strictSkillMatch: false          # If true, ONLY expectedSkills may activate
  continue_session: true           # Continue previous agent session (vs new session)
  verify:
    build: true                    # Run `golem build` after the prompt
    deploy: true                   # Run `golem build` + `golem deploy --yes`
```

#### `shell` — Run a shell command

```yaml
- id: "check-files"
  shell:
    command: "ls"
    args: ["my-app/golem.yaml"]
    cwd: "subdirectory"            # Relative to workspace
  expect:
    exit_code: 0
    stdout_contains: "golem.yaml"
```

#### `http` — Make an HTTP request

```yaml
- id: "call-api"
  http:
    url: "http://my-app.localhost:9006/path"
    method: "POST"                 # GET, POST, PUT, DELETE, PATCH
    headers:
      Content-Type: "application/json"
    body: '{"key": "value"}'
  expect:
    status: 200
    body_contains: "expected text"
    body_matches: "regex.*pattern"
```

#### `invoke` — Invoke a Golem agent function via CLI

```yaml
- id: "call-function"
  invoke:
    agent: 'CounterAgent("my-counter")'
    function: 'app:component/agent.{increment}'
    args: '"hello"'                # Optional function arguments
  expect:
    stdout_contains: "1"
```

#### `invoke_json` — Invoke with `--json` output

Same as `invoke` but passes `--json` flag to `golem agent invoke`. Supports `result_json` assertions with JSONPath.

```yaml
- id: "call-json"
  invoke_json:
    agent: 'MyAgent("test")'
    function: 'app:component/agent.{get-data}'
  expect:
    result_json:
      - path: "$.name"
        equals: "test"
      - path: "$.items[0]"
        contains: "expected"
```

#### `create_agent` — Create a Golem agent

```yaml
- id: "make-agent"
  create_agent:
    name: 'MyAgent("instance-1")'
    env:
      KEY: "value"
    config:
      setting: "value"
```

#### `delete_agent` — Delete a Golem agent

```yaml
- id: "remove-agent"
  delete_agent:
    name: 'MyAgent("instance-1")'
```

#### `trigger` — Fire-and-forget agent function call

```yaml
- id: "trigger-bg"
  trigger:
    agent: 'MyAgent("test")'
    function: 'app:component/agent.{background-task}'
```

#### `sleep` — Wait for a duration

```yaml
- id: "wait"
  sleep: 5  # seconds
```

### Assertions (`expect`)

Available assertion fields:

| Field | Applies To | Description |
|-------|-----------|-------------|
| `exit_code` | shell, invoke | Assert process exit code |
| `stdout_contains` | shell, invoke | Stdout includes substring |
| `stdout_not_contains` | shell, invoke | Stdout must NOT include substring |
| `stdout_matches` | shell, invoke | Stdout matches regex |
| `status` | http | HTTP response status code |
| `body_contains` | http | Response body includes substring |
| `body_matches` | http | Response body matches regex |
| `result_json` | invoke_json | JSONPath assertions on parsed JSON result |

`result_json` entries support:
- `path`: JSONPath expression (e.g., `$.name`, `$.items[0].id`)
- `equals`: Exact match (deep equality)
- `contains`: Substring match on stringified value

### Language-Conditional Fields

`prompt`, `expectedSkills`, `allowedExtraSkills`, and `verify` can be language-conditional:

```yaml
- id: "create-project"
  prompt:
    ts: "Create a new Golem application with TypeScript."
    rust: "Create a new Golem application with Rust."
  expectedSkills:
    ts: ["golem-new-project", "golem-db-app-ts"]
    rust: ["golem-new-project", "golem-db-app-rust"]
```

### Template Variables

Steps support `{{variable}}` substitution. Built-in variables:

| Variable | Value |
|----------|-------|
| `{{workspace}}` | Absolute workspace path |
| `{{scenario}}` | Scenario name |
| `{{agent}}` | Current agent name |
| `{{language}}` | Current language |

## Skill Activation Detection

The harness detects whether an agent actually read a skill using two mechanisms:

1. **Filesystem watcher**: `fswatch` (macOS) or `inotifywait` (Linux) monitors SKILL.md file access events
2. **atime comparison**: Snapshots file access times before each step and compares after

Both mechanisms feed into `expectedSkills` / `allowedExtraSkills` / `strictSkillMatch` verification.

## Agent Drivers

| Agent | CLI Command | Skill Directories | Session Support |
|-------|------------|-------------------|-----------------|
| `claude-code` | `claude --print --permission-mode bypassPermissions` | `.claude/skills/` | Yes (sessionId) |
| `opencode` | `opencode run` | `.claude/skills/`, `.agents/skills/` | No |
| `codex` | `codex exec --dangerously-bypass-approvals-and-sandbox` | `.agents/skills/` | Yes (session_id) |

The driver copies/symlinks all skills from the `--skills` directory into the agent's expected skill directories within the workspace.

## Failure Classification

Failed steps are automatically classified:

| Code | Category | Meaning |
|------|----------|---------|
| `SKILL_NOT_ACTIVATED` | agent | Expected skill was not read by the agent |
| `SKILL_MISMATCH` | agent | Unexpected extra skills were activated |
| `BUILD_FAILED` | build | `golem build` failed |
| `DEPLOY_FAILED` | deploy | `golem deploy` failed |
| `INVOKE_FAILED` | deploy | Agent function invocation failed |
| `INVOKE_JSON_FAILED` | deploy | JSON agent invocation failed |
| `SHELL_FAILED` | infra | Shell command returned non-zero exit |
| `HTTP_FAILED` | network | HTTP request failed or timed out |
| `CREATE_AGENT_FAILED` | infra | `golem agent new` failed |
| `DELETE_AGENT_FAILED` | infra | `golem agent delete` failed |
| `ASSERTION_FAILED` | assertion | Output didn't match expect assertions |

## Output and Reports

Results are written to `--output` (default `./results/`):
- **Per-scenario JSON**: `<agent>-<language>-<scenario-name>.json` with step-by-step results
- **summary.json**: Aggregated pass/fail counts, durations, worst failures
- **report.html**: Visual HTML report
- **GitHub Actions summary**: Auto-generated if `GITHUB_STEP_SUMMARY` is set

## Existing Skills and Scenarios

Skills in `golem-skills/skills/`:
- `golem-new-project` — scaffolding with `golem new`
- `golem-db-app-ts` — database-backed TS application
- `golem-db-app-rust` — database-backed Rust application

Scenarios in `golem-skills/tests/harness/scenarios/`:
- `golem-new-project-ts.yaml` / `golem-new-project-rust.yaml` — project creation
- `golem-build-deploy-ts.yaml` / `golem-build-deploy-rust.yaml` — build + deploy + HTTP verification
- `golem-db-app-ts.yaml` / `golem-db-app-rust.yaml` — database app end-to-end
- `golem-agent-invoke-rust.yaml` — agent function invocation
