---
name: golem-new-project
description: "Creating a new Golem application project. Use when scaffolding a new Golem project with golem new, selecting a language template, and verifying the initial build."
---

# Creating a New Golem Project

**Important: Do not try to build golem from scratch or install it manually.**

Assume the `golem` or `golem-cli` binary exists and is added to PATH.
Try `golem --version` to check if it exists. If not, try `golem-cli --version`. Every command below works for `golem` and `golem-cli`

**Critical rules:**
- Do NOT modify generated SDK dependency versions in files such as `package.json`, `Cargo.toml`, `build.sbt`, or `project/plugins.sbt` unless the user explicitly asks. The generated project is wired to the local checkout and changing it to published versions can break the build.
- For TypeScript projects, do NOT remove or modify generated decorators such as `@agent`, `@endpoint`, `@prompt`, or `@description`. They are valid and required.
- For TypeScript projects, do NOT run `npm install` after `golem new` unless the user explicitly asks. The generated dependencies are already configured correctly.
- If `golem build` fails, read the error carefully. Do NOT try to "fix" it by swapping generated local SDK references to published versions or deleting generated code.
- Keep all file operations inside the current workspace using relative paths (for example, `test-app/golem.yaml`). Do not traverse to parent directories or use absolute paths outside the workspace.
- Prefer shell checks such as `ls`/`find` for generated artifacts in `golem-temp`. Some environments block direct file-tool reads there via ignore rules.

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

Supported template identifiers: `ts` (TypeScript), `rust`, `scala`

## Step 2: Verify Project Structure

After running `golem new`, verify the following:

1. A `golem.yaml` file exists in the project root
2. Language-specific scaffold files exist for the chosen template, for example:
   - TypeScript: `package.json` and `src/main.ts`
   - Rust: `Cargo.toml` and `src/lib.rs`
   - Scala: `build.sbt` and `src/main/scala/...`

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
