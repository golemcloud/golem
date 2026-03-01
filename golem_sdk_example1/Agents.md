# Project Agents.md Guide

This is a [MoonBit](https://docs.moonbitlang.com) project.

## Project Structure

- MoonBit packages are organized per directory, for each directory, there is a
  `moon.pkg.json` file listing its dependencies. Each package has its files and
  blackbox test files (common, ending in `_test.mbt`) and whitebox test files
  (ending in `_wbtest.mbt`).

- In the toplevel directory, this is a `moon.mod.json` file listing about the
  module and some meta information.

## Coding convention

- MoonBit code is organized in block style, each block is separated by `///|`,
  the order of each block is irrelevant. In some refactorings, you can process
  block by block independently.

- Try to keep deprecated blocks in file called `deprecated.mbt` in each
  directory.

## Tooling

- `moon fmt` is used to format your code properly.

- `moon info` is used to update the generated interface of the package, each
  package has a generated interface file `.mbti`, it is a brief formal
  description of the package. If nothing in `.mbti` changes, this means your
  change does not bring the visible changes to the external package users, it is
  typically a safe refactoring.

- In the last step, run `moon info && moon fmt` to update the interface and
  format the code. Check the diffs of `.mbti` file to see if the changes are
  expected.

- Run `moon test` to check the test is passed. MoonBit supports snapshot
  testing, so when your changes indeed change the behavior of the code, you
  should run `moon test --update` to update the snapshot.

- You can run `moon check` to check the code is linted correctly.

- When writing tests, you are encouraged to use `inspect` and run
  `moon test --update` to update the snapshots, only use assertions like
  `assert_eq` when you are in some loops where each snapshot may vary. You can
  use `moon coverage analyze > uncovered.log` to see which parts of your code
  are not covered by tests.

- agent-todo.md has some small tasks that are easy for AI to pick up, agent is
  welcome to finish the tasks and check the box when you are done

## Building and Deploying with Golem CLI

This project uses the `golem` CLI to build, deploy, and test agents on the Golem platform.

### Prerequisites

- `golem` CLI installed (https://github.com/golemcloud/golem/releases)
- `wasm-tools` installed
- A running Golem server (local or cloud)

### Environments and Build Presets

The `golem.yaml` defines two environments:

- **local** — uses `golem server run` on localhost, selects the `debug` build preset
- **cloud** — uses Golem Cloud, selects the `release` build preset

Both presets differ in `moon build` optimization level (`moon build --target wasm` for debug,
`moon build --target wasm --release` for release) and produce output in separate directories.

You can override the preset with `-P <preset>`.

### Starting the Local Server

```shell
golem server run
```

This starts a local Golem server at `http://localhost:9881`. Run it in a separate terminal.

**WARNING**: `golem server run --clean` deletes all existing state. Never run it without asking
the user for confirmation first.

### Building

```shell
# Build with the local (debug) preset:
golem build -L

# Build with the cloud (release) preset:
golem build -E cloud

# Build with an explicit preset override:
golem build -L -P release
```

The build pipeline runs codegen (`reexports` + `agents`), then `moon build`, then
`wasm-tools component embed` and `component new`, then generates and composes the agent wrapper.

Do NOT run `moon build` directly — always use `golem build` which orchestrates the full pipeline.

### Deploying

```shell
# Deploy to local server:
golem deploy -L -Y

# Deploy and reset all existing agents (needed when iterating):
golem deploy -L --reset -Y

# Deploy with release preset to local:
golem deploy -L -P release --reset -Y
```

The `-Y` flag auto-confirms prompts. Use `--reset` to delete existing agent instances — without
it, old agents continue running with the previous component version.

### Invoking Agents

Use `golem agent invoke` to call agent methods. Method names are kebab-case and fully qualified:

```shell
# Format: golem agent invoke -L '<agent-type>(<constructor-args>)' '<component>/<agent-type>.{<method>}' [args...]

# Counter agent — increment, then get value:
golem agent invoke -L 'counter("my-counter")' 'golem:agent-guest/counter.{increment}'
golem agent invoke -L 'counter("my-counter")' 'golem:agent-guest/counter.{get-value}'

# Counter — decrement:
golem agent invoke -L 'counter("my-counter")' 'golem:agent-guest/counter.{decrement}'

# TaskManager — add a task (record argument):
golem agent invoke -L 'task-manager()' \
  'golem:agent-guest/task-manager.{add-task}' \
  '{title: "my task", priority: high, description: some("a description")}'

# TaskManager — get all tasks:
golem agent invoke -L 'task-manager()' 'golem:agent-guest/task-manager.{get-tasks}'

# TaskManager — filter by priority (enum argument):
golem agent invoke -L 'task-manager()' \
  'golem:agent-guest/task-manager.{get-by-priority}' 'high'
```

### Name Mapping (Kebab-Case Convention)

All MoonBit identifiers are converted to **kebab-case** in CLI commands:

- **Agent types**: `Counter` → `counter`, `TaskManager` → `task-manager`
- **Methods**: `get_value` → `get-value`, `add_task` → `add-task`
- **Record fields**: `field_name` → `field-name`
- **Enum cases**: `High` → `high`, `Low` → `low`

### WAVE Value Encoding

Arguments use [WAVE encoding](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave):

| MoonBit Type | WAVE Example |
|---|---|
| `String` | `"hello"` |
| `Bool` | `true`, `false` |
| `UInt64`, `Int`, etc. | `42` |
| `Double`, `Float` | `3.14` |
| `Array[T]` | `[1, 2, 3]` |
| `Option[T]` (Some) | `some("value")` |
| `Option[T]` (None) | `none` |
| `Result[T, E]` | `ok("value")`, `err("msg")` |
| Struct (record) | `{field-name: "value", count: 42}` |
| Simple enum | `high`, `low` |

### Debugging Agents

```shell
golem agent get -L '<agent-id>'          # Check agent state
golem agent stream -L '<agent-id>'       # Stream live logs
golem agent oplog -L '<agent-id>'        # View operation log
```
