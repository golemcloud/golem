---
name: golem-add-component
description: "Adding a new component or agent templates to an existing Golem application. Use when adding a second component, adding agent templates like human-in-the-loop or snapshotting to an existing component, or converting a single-component app to multi-component."
---

# Adding Components and Agent Templates to an Existing Golem Application

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Two Modes of `golem new` in an Existing Application

When `golem new` is run inside an existing application directory (one that already has a `golem.yaml`), it operates in **existing application mode**. There are two use cases:

### 1. Add a new component (with `--component-name`)

Creates a new component in the application. The component gets its own directory, source files, and entry in `golem.yaml`.

```shell
golem new --template <TEMPLATE> --component-name <NAMESPACE:NAME> --yes .
```

### 2. Add agent templates to an existing component (without `--component-name`)

Adds new agent template files to an existing component. The CLI automatically detects which component to target based on the template's language. If there are multiple components using the same language, specify `--component-name` to disambiguate.

```shell
golem new --template <TEMPLATE> --yes .
```

## Template Names

Templates are specified as `<language>` (for the default template) or `<language>/<name>` (for named templates).

### Rust templates

| Template | Description |
|----------|-------------|
| `rust` | Default — a simple counter agent |
| `rust/human-in-the-loop` | Agent using promises for human-in-the-loop workflows |
| `rust/json` | Agent demonstrating JSON API support |
| `rust/snapshotting` | Agent with custom state snapshotting |

### TypeScript templates

| Template | Description |
|----------|-------------|
| `ts` | Default — a simple counter agent |
| `ts/human-in-the-loop` | Agent using promises for human-in-the-loop workflows |
| `ts/json` | Agent demonstrating JSON API support |
| `ts/snapshotting` | Agent with custom state snapshotting |

### Scala templates

| Template | Description |
|----------|-------------|
| `scala` | Default — a simple counter agent |

## Required Flags

| Flag | Description |
|------|-------------|
| `--template <TEMPLATE>` | Template to apply (see template names above). |
| `--yes` / `-Y` | Non-interactive mode. **Always use this flag.** |

## Optional Flags

| Flag | Description |
|------|-------------|
| `--component-name <NAMESPACE:NAME>` | Target component. Required when adding a new component. Also required when there are multiple existing components using the same language as the template. |

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

## Agent Name Conflicts

Default templates all create an agent named `CounterAgent`. When adding a new component using a default template (e.g., `rust`, `ts`, `scala`), the generated agent will conflict with any existing `CounterAgent` in other components, causing deployment failures.

After adding a new component with a default template, **rename the generated agent** (both the type/trait name and the HTTP mount path in the source files) to something unique before building or deploying.

## After Adding a Component or Template

1. **Build** to verify the new component compiles:
   ```shell
   golem build
   ```

2. **Deploy** to make it available:
   ```shell
   golem deploy --reset --yes
   ```

3. For cross-component communication, load the relevant RPC skill (`golem-call-another-agent-rust`, `golem-call-another-agent-ts`, or `golem-call-another-agent-scala`).
