---
name: golem-rollback
description: "Rolling back a Golem deployment to a previous revision or version. Use when reverting a deployment, restoring a prior environment state, or recovering from a bad deploy."
---

# Rolling Back a Deployment

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Overview

Every `golem deploy` creates a new **deployment revision** — an immutable snapshot of the environment's components and HTTP API deployments. Rollback lets you revert the environment to a previous revision, restoring the exact component versions and API configuration that were active at that point.

Rollback does **not** rebuild or re-upload components — it re-activates a previously deployed revision on the server side.

## Rollback by Revision

```shell
golem deploy --revision <REVISION_NUMBER> --yes
```

Reverts the environment to the exact state captured at the given deployment revision number. The revision must exist in the environment's deployment history.

If the specified revision is not found, the CLI prints an error and lists all available deployments with their revision numbers and versions.

## Rollback by Version

```shell
golem deploy --version <VERSION_STRING> --yes
```

Looks up the deployment whose version label matches `<VERSION_STRING>` and rolls back to that revision. If multiple deployments share the same version string, the CLI reports an error and asks you to use `--revision` instead.

## How Rollback Works

1. **Prepare** — the CLI fetches the target revision's deployment summary from the server and diffs it against the current deployment.
2. **Diff** — a unified diff is displayed showing exactly which components, component versions, environment variables, and HTTP API deployments will change.
3. **Confirm** — unless `--yes` is passed, the CLI prompts for confirmation before proceeding.
4. **Apply** — the CLI sends a `DeploymentRollback` request to the server containing the current revision and the target revision. The server atomically switches the environment to the target state.

If the target revision is identical to the current deployment (same deployment hash), the CLI reports "up to date" and takes no action.

## Combining Rollback with Post-Deploy Actions

Rollback supports the same post-deploy flags as a normal deploy:

| Flag | Description |
|------|-------------|
| `--update-agents <MODE>` | Update existing agents to the rolled-back component version (`auto` or `manual`) |
| `--redeploy-agents` | Delete and recreate existing agents using the rolled-back version |
| `--reset` | Delete agents and the environment, then deploy |

### Examples

Roll back and update all running agents automatically:

```shell
golem deploy --revision 3 --update-agents auto --yes
```

Roll back by version and redeploy agents:

```shell
golem deploy --version "v1.2.0" --redeploy-agents --yes
```

## Planning a Rollback (Dry Run)

```shell
golem deploy --revision 3 --plan
```

Shows the diff of what would change without applying anything. Useful for inspecting the impact before committing.

## Listing Available Revisions

If you do not know which revision to target, deploy with a non-existent revision number — the CLI will print all available deployments. Alternatively, use the environment API or the Golem dashboard to browse deployment history.

## Constraints

- `--revision` and `--version` conflict with each other — use one or the other.
- `--revision` and `--version` conflict with `--force-build`, `--stage`, and `--approve-staging-steps` — rollback does not trigger a build.
- The environment must already have at least one deployment (a current deployment must exist) before a rollback can be performed.
- Rollback is an environment-level operation — it affects all components in the environment, not individual components.
