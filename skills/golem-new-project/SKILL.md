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
golem new my-app typescript
```

Use the `-Y` flag for non-interactive mode (accepts all defaults):

```shell
golem new my-app typescript -Y
```

### Supported Languages

Supported language identifiers: `typescript`, `rust`

## Step 2: Verify Project Structure

After running `golem new`, verify the following:

1. A `golem.yaml` file exists in the project root
2. A folder named common-{ts|rust} exists in the project root

## Step 3: Build the Project

```shell
golem build
```
## Checklist

1. `golem new <name> <language>` executed successfully
2. `golem.yaml` exists in the project root
3. Source files are present for the chosen language
4. `golem build` succeeds without errors
