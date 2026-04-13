---
name: golem-deploy
description: "Deploying a Golem application. Use when asked to deploy agents to a Golem server, update running agents, or troubleshoot deployment issues."
---

# Deploying a Golem Application with `golem deploy`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem deploy --yes
```

Run this from the application root directory (where the root `golem.yaml` is located). It automatically builds all components (with up-to-date checks), uploads them to the configured Golem server, and creates or updates the deployment. **Always pass `--yes`** to avoid interactive prompts.

## What `golem deploy` Does

1. **Builds** — automatically builds all components if needed, with proper up-to-date checks (same as `golem build`).
2. **Stages** — compares local component artifacts with what is currently deployed on the server.
3. **Uploads** — pushes new or changed component versions to the server.
4. **Deploys** — activates the uploaded components so agents can be created and invoked.

Existing running agents are **not affected** by default — they continue running with their previous component version. Use `--update-agents` or `--redeploy-agents` to change this behavior.

## Available Options

| Option | Description |
|--------|-------------|
| `--plan` | Only plan deployment, apply no changes |
| `--force-build` | Skip modification-time based up-to-date checks |
| `-u, --update-agents <MODE>` | Update existing agents: `automatic` or `manual` |
| `--redeploy-agents` | Delete and recreate existing agents |
| `-r, --reset` | Delete agents and the environment, then deploy from scratch |
| `-P, --preset <PRESET>` | Select custom component presets |
| `-Y, --yes` | Non-interactive mode — **always use this flag** |

### Environment Selection

| Option | Description |
|--------|-------------|
| `-E, --environment <NAME>` | Select a Golem environment by name |
| `-L, --local` | Use `local` environment or profile |
| `-C, --cloud` | Use `cloud` environment or profile |
| `--profile <PROFILE>` | Select a Golem profile by name |

## Deployment Strategies

### Fresh deployment

```shell
golem deploy --yes
```

Builds, uploads, and activates all components. Agents are created on first invocation.

### Iterative development (recommended)

```shell
golem deploy --yes --reset
```

Deletes all previously created agents and redeploys everything. Use this when iterating on code changes — without it, existing agent instances keep running with the old component version.

### Update running agents in-place

```shell
golem deploy --yes --update-agents automatic
```

Updates existing agents to the new component version automatically. Agents pick up the new code on their next invocation.

### Preview changes without applying

```shell
golem deploy --yes --plan
```

Shows what would be deployed without making any changes.

## Common Deployment Errors

- **Build failure**: fix the build errors reported in the output — `golem deploy` runs the build automatically.
- **Server not reachable**: ensure the Golem server is running and accessible at the configured address.
- **Port conflict**: if deploying locally, make sure no other process is using the Golem server ports.
