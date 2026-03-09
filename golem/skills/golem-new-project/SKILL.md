---
name: golem-new-project
description: "Creating a new Golem application project. Use when scaffolding a new Golem project with golem new, selecting a language template, and verifying the initial build."
---

# Creating a New Golem Project

Use the `golem new` CLI command to scaffold a new Golem application with the desired language template.

## Step 1: Run `golem new`

```shell
golem new <APPLICATION_NAME> <LANGUAGE>
```

For example, to create a TypeScript project:

```shell
golem new my-app ts
```

Use the `-Y` flag for non-interactive mode (accepts all defaults):

```shell
golem new my-app ts -Y
```

### Supported Languages

Common language identifiers: `ts` (TypeScript), `rust`, `go`, `python`, `js` (JavaScript), `zig`, `c`, `csharp`.

## Step 2: Verify Project Structure

After running `golem new`, verify the following:

1. A `golem.yaml` file exists in the project root
2. The project directory contains the expected language-specific source files
3. Component directories are created under `src/`

## Step 3: Build the Project

```shell
golem build
```

This compiles all components defined in `golem.yaml` and produces the WASM artifacts.

## Checklist

1. `golem new <name> <language>` executed successfully
2. `golem.yaml` exists in the project root
3. Source files are present for the chosen language
4. `golem build` succeeds without errors
