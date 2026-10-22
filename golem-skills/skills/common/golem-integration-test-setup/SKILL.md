---
name: golem-integration-test-setup
description: "Setting up a dedicated Golem environment for integration testing. Use when adding integration tests for a Golem application, configuring a throwaway local server for tests, isolating test state from local development, or wiring up CI to run end-to-end tests against a real Golem server."
---

# Setting Up a Golem Environment for Integration Testing

Integration tests for a Golem application typically need:

- a **clean, isolated Golem server** that does not share state with local development or other test runs
- a **dedicated environment** in `golem.yaml` so tests can deploy and target their own configuration
- **non-interactive deploys** so the test harness can drive `golem deploy` without prompts
- **predictable or discoverable ports** so the test runner knows where to send HTTP / MCP / gRPC requests

This skill describes the recommended way to wire all of this together.

## Recommended Layout

```
my-app/
├── golem.yaml
├── components/...
└── tests/
    ├── integration/
    │   └── ...                   # Test sources
    └── fixtures/
        ├── data/                 # Server data dir (gitignored)
        ├── ports.json            # Written by `golem server run --ports-file`
        └── golem-server.log
```

Add the test fixture paths to `.gitignore`:

```gitignore
tests/fixtures/data/
tests/fixtures/ports.json
tests/fixtures/golem-server.log
```

## 1. Add a Test Environment to `golem.yaml`

Add a dedicated environment (commonly named `test` or `integration`) alongside `local` and `cloud`. Point it at the built-in `local` server, activate test-specific presets, and turn on the CLI options that make deploys non-interactive.

```yaml
environments:
  local:
    default: true
    server: local
    componentPresets: dev

  test:
    server: local                  # Tests run against a local `golem server`
    componentPresets: test         # Activate the "test" preset on every component/agent
    cli:
      format: json                 # Machine-readable CLI output for the test harness
      autoConfirm: true            # Skip interactive prompts (equivalent to --yes)
      reset: true                  # Always start from a clean deployment (equivalent to --reset)
    deployment:
      compatibilityCheck: false    # Speed up deploys during tests
      versionCheck: false

  cloud:
    server: cloud
    componentPresets: prod
```

Key choices:

- **`server: local`** — tests run against a local server you start and stop yourself, never against `cloud`.
- **`cli.autoConfirm: true` and `cli.reset: true`** — every `golem -E test deploy` becomes equivalent to `golem deploy --yes --reset`, deploying and then deleting leftover agents. Guest KV data remains unless the application clears it separately.
- **`cli.format: json`** — lets the test harness parse CLI output reliably.
- **`deployment.compatibilityCheck: false` / `versionCheck: false`** — these checks are valuable in production but slow down rapid test iteration on a throwaway server.

## 2. Add a `test` Preset to Components and Agents

Use a component/agent preset to inject test-only configuration: shorter timeouts, in-memory fakes, test API keys, debug logging, etc. The `componentPresets: test` line in the environment activates it.

```yaml
componentTemplates:
  shared:
    env:
      LOG_LEVEL: info
    presets:
      dev:
        env:
          LOG_LEVEL: debug
      test:
        env:
          LOG_LEVEL: debug
          GOLEM_ENV: test

agents:
  MyAgent:
    env:
      CACHE_TTL: "300"
      EXTERNAL_API_URL: "https://api.example.com"
    presets:
      test:
        env:
          CACHE_TTL: "1"           # Effectively disable caching in tests
          EXTERNAL_API_URL: "http://localhost:0"   # Overridden at test-runtime
```

> **Naming guideline:** prefer distinct preset names (`dev`, `test`, `release`) over reusing environment names. See `golem-profiles-and-environments` for the rationale.

## 3. Start an Isolated Local Server for the Test Run

Run each test server with its own unique data directory and ports file (the paths below assume one run at a time). Remove any stale ports file before starting and let the server pick free ports. This ensures that:

- Tests never collide with a developer's local server (which uses the platform-specific default data dir and ports `9881` / `9006` / `9007`).
- Multiple test runs (locally or in CI) can execute in parallel.
- Each test run starts from a known empty state.

```shell
golem server run \
  --data-dir ./tests/fixtures/data \
  --router-port 0 \
  --custom-request-port 0 \
  --mcp-port 0 \
  --ports-file ./tests/fixtures/ports.json \
  --clean --yes
```

| Flag | Why it matters for tests |
|------|--------------------------|
| `--data-dir <path>` | Keep test state out of the developer's default data directory |
| `--clean` | Wipe the data directory before starting so each run is reproducible |
| `--router-port 0` (and friends) | Let the OS assign free ports — works in parallel CI |
| `--ports-file <path>` | Server writes the actual bound ports here once it is fully ready |

The server runs in the foreground and blocks the terminal — start it as a background process from the test harness and tear it down after the suite finishes.

> **Only the `golem` binary supports `golem server`** — `golem-cli` does not.

## 4. Wait for the Server, Then Read the Ports

The `--ports-file` is written **atomically once all services are ready**, so polling for its existence is a reliable readiness signal. After it appears, parse it to discover the ports:

```json
{
  "routerPort": 51823,
  "customRequestPort": 51824,
  "mcpPort": 51825
}
```

| Field | Used for |
|-------|----------|
| `routerPort` | `golem deploy`, `golem agent invoke`, gRPC / management API |
| `customRequestPort` | HTTP API endpoints exposed by the application |
| `mcpPort` | MCP server requests |

The CLI does **not** read `ports.json` automatically. Before any deploy or agent CLI command, set the built-in local URL in the harness environment and propagate it to every CLI subprocess:

```shell
export GOLEM_BUILTIN_LOCAL_URL="http://127.0.0.1:$(jq -er '.routerPort' tests/fixtures/ports.json)"
```

For random HTTP/MCP ports, generate an included manifest fragment **after discovery and before deploy** with `localServer.customRequestPort` and `localServer.mcpPort` set to the actual discovered nonzero values. Subdomain expansion then matches the running listeners. The router URL override above remains necessary; writing a ports file alone does not configure CLI connections.

For example, in an isolated test copy of the application, include `golem-test-ports.json` from the root `golem.yaml` and generate that fragment with:

```shell
jq -e '{manifestVersion: "1.6.0", localServer: {customRequestPort: .customRequestPort, mcpPort: .mcpPort}}' \
  tests/fixtures/ports.json > golem-test-ports.json
```

JSON is valid YAML. Every included fragment requires `manifestVersion: "1.6.0"`. Create the included file with `{"manifestVersion":"1.6.0"}` before starting the server so manifest loading succeeds, then replace it with the discovered ports before deploying. Do not define `localServer` elsewhere in this test manifest. Parallel runs need separate application copies or manifests as well as unique data paths, so they do not overwrite each other's fragment. Remove the generated fragment during teardown.

Alternatively, for a single non-parallel test run, use stable nonzero manifest ports and start the server with those same ports (omit the three port-0 flags in step 3):

```yaml
localServer:
  routerPort: 9881
  customRequestPort: 9006
  mcpPort: 9007

environments:
  test:
    server: local
    componentPresets: test
    cli:
      autoConfirm: true
      reset: true
```

`customRequestPort` and `mcpPort` make deployment `subdomain` values resolve to the configured HTTP API and MCP ports instead of the defaults. Do not set any manifest `localServer` port field to `0`; port `0` is only allowed when passed directly as a `golem server run` port flag.

If it helps the test workflow to keep local server settings separate from the main manifest, use an included manifest fragment:

```yaml
includes:
  - golem-local-server.yaml
```

Then define `localServer` in `golem-local-server.yaml`:

```yaml
manifestVersion: "1.6.0"

localServer:
  routerPort: 9881
  customRequestPort: 9006
  mcpPort: 9007
  portsFile: .golem/ports.json
  dataDir: .golem/data
```

`localServer` is a singleton across all manifest sources, so define it either in the main manifest or in the included file, not both. Include paths are relative to the manifest that declares `includes`; `localServer` path fields are relative to the manifest that declares `localServer`. If you select the manifest explicitly with `-A` / `--app-manifest-path`, pass the root `golem.yaml`; its `includes` are followed the same way as during auto-discovery.

## 5. Deploy Against the Test Environment

From the application root, target the test environment by name:

```shell
golem -E test deploy
```

With `GOLEM_BUILTIN_LOCAL_URL` exported from step 4, this deploy targets the discovered router. Because the environment sets `cli.autoConfirm: true` and `cli.reset: true`, this is equivalent to `golem deploy --yes --reset` — no agents carry over from earlier runs. Reset does not clear guest KV buckets; clear application KV separately or use a new server data directory for full isolation.

For faster iteration during test development, reuse the same server process across multiple deploys. `--reset` deploys, then deletes existing agents of the deployed components; it does not delete the environment or restart the server. It does not clear domains, HTTP and MCP deployments, secrets, retry policies, plugins, or component revision history. The deployment still reconciles the manifest and can update these resources, including replacement of incompatible agent secrets.

## 6. Send Requests From Tests

Inside the test code, invoke agents either via the CLI or by calling the HTTP / MCP endpoints directly:

- **Management / invocation API** — `http://127.0.0.1:<routerPort>` (also where `golem -E test agent invoke ...` connects when it inherits `GOLEM_BUILTIN_LOCAL_URL`).
- **HTTP API endpoints** — `http://<deployment-subdomain>.localhost:<customRequestPort>/<your-route>`, for example `http://test-api.localhost:<customRequestPort>/<your-route>`.
- **MCP** — `http://<deployment-subdomain>.localhost:<mcpPort>/mcp`, for example `http://test-mcp.localhost:<mcpPort>/mcp`.

Use the `httpApi.deployments.test`, `mcp.deployments.test`, `secretDefaults.test`, and `retryPolicyDefaults.test` sections of `golem.yaml` to provide test-specific routing and configuration; these sections are keyed by environment name.

```yaml
httpApi:
  deployments:
    test:
      - subdomain: test-api  # resolves to test-api.localhost:<customRequestPort>
        agents:
          MyAgent: {}

mcp:
  deployments:
    test:
      - subdomain: test-mcp  # resolves to test-mcp.localhost:<mcpPort>
        agents:
          MyAgent: {}

secretDefaults:
  test:
    apiKey: "test-key"
```

## 7. Tear Down

Register teardown before spawning the server and run it in a `finally` block, including after startup, deploy, or test failure:

1. Send `SIGINT` / `SIGTERM` to the `golem server run` process.
2. Await and reap the child with a bounded shutdown timeout. If it does not exit, force-kill it and await/reap it before proceeding. Do not remove files while the server can still write them.
3. Only after the child has exited, delete the isolated test data directory, ports file, and generated manifest fragment. If using `golem server clean` instead, set `localServer.dataDir` to that isolated directory and verify the resolved path before passing `--yes`.

Do **not** run `golem server clean -X` from a test — that would delete the developer's default local development state. Keep test data isolated with manifest `localServer.dataDir`.

## End-to-End Skeleton

Pseudocode for a typical test harness:

```text
allocate a unique run directory and an isolated application copy
set data_dir and ports_file inside that run directory
create the included fragment with {"manifestVersion":"1.6.0"}
child = none
try:
    spawn child: golem server run --data-dir <data_dir> \
                    --router-port 0 --custom-request-port 0 --mcp-port 0 \
                    --ports-file <ports_file> --clean --yes
    deadline = monotonic_now + startup_timeout
    loop:
        if child has exited: fail with exit status and server log
        if monotonic_now >= deadline: fail with startup timeout and server log
        if ports_file exists: break
        wait briefly
    read ports; validate all three are nonzero
    set GOLEM_BUILTIN_LOCAL_URL to http://127.0.0.1:<routerPort> for every CLI subprocess
    generate the included fragment with manifestVersion "1.6.0" and discovered HTTP/MCP ports
    run golem -E test deploy from the isolated application copy
    run tests against discovered ports and matching subdomain Host headers
finally:
    if child exists:
        if child is alive: send SIGINT or SIGTERM
        await/reap child with shutdown_timeout
        on timeout: force-kill child; await/reap it
    only after child exit (or spawn failure), remove data_dir, ports_file, and fragment
```

## Common Pitfalls

- **Sharing state with local dev** — forgetting `--data-dir` makes tests delete or pollute the developer's default data directory. Always pass `--data-dir` for tests.
- **Race against startup** — sending requests before the server is ready. Use the `--ports-file` as the readiness signal; do not just `sleep`.
- **Forgetting `cli.reset: true` (or `--reset`)** — old agents survive across test runs and silently serve stale code.
- **Hard-coded ports in CI** — two test jobs on the same runner collide. Always bind to port `0` and read `ports.json`.
- **Missing test preset** — without activating a `test` preset, agents run with production environment variables and external endpoints.
- **Pointing `server:` at `cloud`** — never set the test environment to `cloud`. Tests should run against an isolated local server only.

## Related Skills

- Load `golem-local-dev-server` for the full reference of `golem server` flags and the data directory layout
- Load `golem-profiles-and-environments` for how environments, presets, and profiles interact
- Load `golem-edit-manifest` for the complete `golem.yaml` field reference
- Load `golem-deploy` for deployment command details and flags
- Load `golem-test-crash-recovery` for testing durable execution and crash recovery
