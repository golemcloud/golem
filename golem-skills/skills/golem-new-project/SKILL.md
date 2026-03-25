---
name: golem-new-project
description: "Creating a new Golem application project. Use when scaffolding a new Golem project with golem new, selecting a language template, and verifying the initial build."
---

# Creating a New Golem Project

**Important: Do not try to build golem from scratch or install it manually.**

Assume the `golem` or `golem-cli` binary exists and is added to PATH.
Try `golem --version` to check if it exists. If not, try `golem-cli --version`. Every command below works for `golem` and `golem-cli`

## Step 1: Run `golem new`

```shell
golem new <APPLICATION_NAME> --template <TEMPLATE> -Y
```

The `-Y` flag enables non-interactive mode (accepts all defaults). Always use it.

For example, to create a TypeScript project:

```shell
golem new my-app --template ts -Y
```

### Supported Templates

Supported template identifiers: `ts` (TypeScript), `rust`

## Step 2: Verify Project Structure

After running `golem new`, verify the following:

1. A `golem.yaml` file exists in the project root
2. A folder named common-{ts|rust} exists in the project root

## Step 3: Build the Project

```shell
cd <APPLICATION_NAME>
golem build
```
## Checklist

1. `golem new <name> --template <template> -Y` executed successfully
2. `golem.yaml` exists in the project root
3. Source files are present for the chosen language
4. `golem build` succeeds without errors
