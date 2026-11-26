<!-- golem-managed:guide:python:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

**Activation cues for `golem.yaml` edits**: whenever a task involves editing `golem.yaml`, load `golem-edit-manifest` for the manifest schema, and also load the section-specific skill — `golem-add-env-vars` for `env`/`envDefaults`/`secretDefaults` changes, `golem-add-initial-files` for `files:` blocks, `golem-profiles-and-environments` for `presets`/environment-scoped sections, `golem-manage-plugins` for `plugins:` entries, `golem-configure-api-domain` for `httpApi`, and `golem-configure-mcp-server` for `mcp`.

| Skill | Description |
|-------|-------------|
| `golem-cloud-account-setup` | Setting up a Golem Cloud account — authentication, cloud profiles, API tokens, and first cloud deployment |
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) — components, agents, templates, environments, httpApi, mcp, bridge SDKs, plugins, and more |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-troubleshoot-build` | Troubleshooting Golem build failures and debugging manifest file (golem.yaml) configuration — diagnosing tool, dependency, env var, config, and manifest layer issues with `golem component manifest-trace` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-local-dev-server` | Starting, configuring, and debugging the local Golem development server with `golem server` — verbosity flags, useful tracing targets, and key log lines |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-integration-test-setup` | Setting up a dedicated Golem environment for integration testing — isolated local server, test environment in golem.yaml, dynamic port discovery, and non-interactive deploys |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |

# Golem Application Development Guide (Python)

## Overview

This is a **Golem Application** whose components are written in Python. The Golem CLI assembles each component from the modules in its `src` directory and the `golem-py-sdk` wheel, which contains the SDK and a prepared CPython runtime. The build runs no Python, uv or C compiler on the host.

The Python SDK is in development. It supports only what this guide lists.

## Agents

```python
from golem import agent, method


@agent
class CounterAgent:
    def __init__(self, name: str) -> None:
        self.name = name
        self.value = 0

    @method
    def increment(self, amount: int) -> int:
        self.value += amount
        return self.value
```

- `@agent` declares an agent type with the class name. The `__init__` parameters are the constructor parameters, which identify the agent.
- `@method` exports a method with the function name. Other methods are not exported.
- An agent keeps its object state between calls. Golem restores the state after a crash.
- `src/main.py` is the entry module: import every module that declares agents there.

## Supported types and limits

- Parameter and result types: `bool`, `int` (`s64`), `float` (`f64`), `str`, `bytes` and, for results, `None`. Every parameter and the result need an annotation.
- Methods are synchronous. Async and generator methods, default, variadic and keyword-only parameters, compound types, config, descriptions and HTTP mounts are not supported yet. The build fails with the location of each unsupported declaration.
- `pyproject.toml` may list only `golem-py-sdk` in `project.dependencies`. Third-party packages are not supported yet; the build fails when they are listed.
- `pyproject.toml` may not declare package sources: `[tool.uv.sources]` and the uv `index`, `index-url`, `extra-index-url` and `find-links` settings fail the build. To build with a local SDK wheel, set `GOLEM_PYTHON_SDK_WHEEL`.
- An exception from a constructor or a method fails the invocation with its message.

## Building

- `golem build` — assemble the components
- `golem deploy` — deploy them

The `golem-py-sdk` wheel is not published yet. Build it in a Golem checkout with `cargo make python-sdk-wheel` and set `GOLEM_PYTHON_SDK_WHEEL` to its path.

## Running Golem CLI commands non-interactively

In non-interactive contexts (CI, scripts, coding agents) **always pass `--yes` (or `-y`) to mutating commands**, such as `golem build --yes` and `golem deploy --yes`.

## Documentation

- Golem docs: https://learn.golem.cloud
- App manifest reference: https://learn.golem.cloud/app-manifest
<!-- golem-managed:guide:python:end -->
