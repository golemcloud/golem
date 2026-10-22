---
name: golem-add-component
description: "Adding a new component or agent templates to an existing Golem application. Use when adding a second component, adding agent templates like human-in-the-loop or snapshotting to an existing component, or converting a single-component app to multi-component."
---

# Adding Components and Agent Templates to an Existing Golem Application

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Two Modes of `golem new` in an Existing Application

When `golem new` is run inside an existing application directory (one that already has a `golem.yaml`), it operates in **existing application mode**. There are two use cases:

### 1. Select a component explicitly (with `--component-name`)

An existing name extends that component with the template (the language must match). A new name creates a component with its own directory, source files, and entry in `golem.yaml`.

```shell
golem new --template <TEMPLATE> --component-name <NAMESPACE:NAME> --yes .
```

### 2. Select by template language (without `--component-name`)

With no matching component, the CLI creates `<application>:<lang>-main`. With one matching component, it adds the template there. With multiple matches, interactive mode asks you to choose; with `--yes`, specify `--component-name` to disambiguate.

```shell
golem new --template <TEMPLATE> --yes .
```

## Template Names

Templates are specified as `<language>` (for the default template) or `<language>/<name>` (for named templates).

The language IDs are `rust`, `ts` (TypeScript), `effect`, `scala`, `moonbit`, and `go`.
All six languages provide the same core examples:

| Template | Agent types | Description |
|----------|-------------|-------------|
| `<language>` | `CounterAgent` | A named counter with `increment`, `value`, `add`, and `reset` methods |
| `<language>/json` | `TaskAgent` | A task manager with JSON object input and output |
| `<language>/human-in-the-loop` | `WorkflowAgent`, `HumanAgent` | A named workflow that waits for a human approval decision |
| `<language>/streaming` | `StreamingAgent` | Agent streams, nested streams, and durable streams |

For example, use `effect/json` for an Effect task manager or `go/human-in-the-loop` for a Go approval workflow.

Rust, TypeScript, Effect, Scala, and MoonBit also provide `<language>/snapshotting`.
This template defines `CounterWithSnapshotAgent`, which snapshots its state every five invocations.
There is no `go/snapshotting` template. Go supports custom snapshot recovery through `Snapshotter`, but default JSON recovery and manual snapshot-based updates have separate limitations. See `golem-custom-snapshot-go` before enabling snapshots.

TypeScript also provides `ts/counter-arktype` and `ts/counter-valibot` to demonstrate alternative schema libraries.

Agent type names are unique within an environment, including across components. If you add the same
example in more than one language to one environment, rename the colliding agent types and their
references in `golem.yaml`. Different component names alone do not prevent agent type name collisions.

Use `golem templates` to list the available templates.

## Required Flags

| Flag | Description |
|------|-------------|
| `--template <TEMPLATE>` | Template to apply (see template names above). |
| `--yes` / `-Y` | Non-interactive mode. **Always use this flag.** |

## Optional Flags

| Flag | Description |
|------|-------------|
| `--component-name <NAMESPACE:NAME>` | Existing name extends that component; new name creates one. Required with `--yes` when multiple components match the template language. |

## Examples

### Add a new Rust component to an existing application

```shell
golem new --template rust --component-name myapp:billing-service --yes .
```

This creates a new `billing-service/` directory with Rust component scaffolding and adds a `myapp:billing-service` entry to `golem.yaml`.

### Add a new TypeScript component alongside an existing Rust component

```shell
golem new --template ts --component-name myapp:frontend --yes .
```

### Add the human-in-the-loop template to an existing Rust component

```shell
golem new --template rust/human-in-the-loop --yes .
```

When there is only one Rust component, the CLI automatically targets it.

### Add a template to a specific component (multi-component app)

```shell
golem new --template rust/snapshotting --component-name myapp:orders --yes .
```

## Single-Component to Multi-Component Promotion

When the existing application has a single component with `dir: "."` (source files in the root), adding a second component triggers an **automatic layout migration**:

1. The CLI creates a new subdirectory for the existing component (e.g., `rust-main/`)
2. It moves the existing component's source files into that subdirectory
3. It creates a new subdirectory for the new component
4. It updates `golem.yaml` to reflect the new directory layout

The CLI shows the planned file moves and asks for confirmation (auto-confirmed with `--yes`).

> **Go:** a single Go component lives in a `module/` directory (`dir: "module"`), not at the app root (its `go.mod` stays sealed away from `golem.yaml`/`golem-temp`). Promotion renames that directory to the component's name (e.g. `module/` → `go-main/`) and creates a sibling directory for the new component.

## Agent Name Conflicts

Default templates all create an agent named `CounterAgent`. When adding a new component using a default template (e.g., `rust`, `ts`, `scala`), the generated agent will conflict with any existing `CounterAgent` in other components, causing deployment failures.

After adding a new component with a default template, **rename the generated agent** (both the type/trait name and the HTTP mount path in the source files) to something unique before building or deploying.

## After Adding a Component or Template

1. **Build** to verify the new component compiles:
   ```shell
   golem build --yes
   ```

2. **Deploy** to make it available:
   ```shell
   golem deploy --yes
   ```

   Only add `--reset` for explicitly disposable instances: it deletes all existing durable agents across the deployed components and loses their state, not just agents of the new component.

3. For cross-component communication, load the relevant RPC skill (`golem-call-another-agent-rust`, `golem-call-another-agent-ts`, or `golem-call-another-agent-scala`).
