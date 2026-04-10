---
name: golem-new-project
description: "Creating a new Golem application project. Use when scaffolding a new Golem project with golem new, selecting a language template, and setting up the initial project structure."
---

# Creating a New Golem Project with `golem new`

Both `golem` and `golem-cli` can be used — all commands below work with either binary. The `golem` binary is a superset that includes a built-in local server.

## Usage

Always use non-interactive mode by passing `--yes` (or `-Y`) and specifying `--template`:

```shell
golem new --template <LANGUAGE> --yes <APPLICATION_PATH>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<APPLICATION_PATH>` | Directory where the new application is created. The folder name becomes the application name by default. |

### Required flags

| Flag | Description |
|------|-------------|
| `--template <LANGUAGE>` | Language template to use. See supported languages below. |
| `--yes` / `-Y` | Non-interactive mode — automatically accepts all confirmation prompts. **Always use this flag.** |

### Optional flags

| Flag | Description |
|------|-------------|
| `--application-name <NAME>` | Override the application name (defaults to the folder name). |
| `--component-name <NAMESPACE:NAME>` | Set a specific component name. Must follow `namespace:name` format. Defaults to a name derived from the application name and language. |
| `--preset <PRESET>` | Select a component preset. Generated projects come with `debug` and `release` presets by default (configured in `golem.yaml`). |

## Supported languages

| Language | Template value |
|----------|---------------|
| Rust | `rust` |
| TypeScript | `ts` |
| Scala | `scala` |

## Examples

Create a new Rust project:
```shell
golem new --template rust --yes my-rust-app
```

Create a new TypeScript project:
```shell
golem new --template ts --yes my-ts-app
```

Create a new Scala project:
```shell
golem new --template scala --yes my-scala-app
```

Create a project with a custom application name:
```shell
golem new --template rust --application-name my-app --yes ./projects/my-app
```

Create a project with a specific component name:
```shell
golem new --template ts --component-name myns:my-component --yes my-app
```

Create a project with the release preset:
```shell
golem new --template rust --preset release --yes my-app
```
