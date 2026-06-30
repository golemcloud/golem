---
name: golem-cli-development
description: "General development in cli/golem-cli, especially command handlers, command wiring, TUI integration with CLI handlers, and CLI handler refactors that are not schema-specific."
---

# Golem CLI Development

Use this skill for general `cli/golem-cli` work that is not specifically about
structured output schema or manifest schema changes.

For structured output types, `CliOutput`, or `command-output.schema.json`, use
`modifying-cli-output-schema`. For application manifest schema versions, use
`modifying-cli-manifest-schema`.

## Command Handler Layout

CLI command handlers follow a strict method order:

1. `new`
2. `handle_command` or the handler entrypoint
3. `cmd_xxx` command methods
4. helper methods

Keep new and moved methods in this order when editing handler modules.

## Handler Design Rules

- Keep `cmd_xxx` methods focused on command flow: validate command-specific
  arguments, call shared logic, and render/log results.
- Extract neutral data-returning helpers when behavior is shared by CLI, TUI, or
  tests. Do not add caller-specific helpers such as `for_tui`.
- Keep human rendering, structured output, logging, prompts, and exits at the
  command edge. Shared helpers should return typed data or clear domain results.
- Reuse existing handler factories through `Handlers` instead of constructing
  cross-handler dependencies ad hoc.
- Preserve local module style before introducing new abstractions.

## Validation

For focused CLI handler changes, usually run:

```shell
cargo fmt --package golem-cli
cargo check -p golem-cli
cargo test -p golem-cli --lib -- <relevant_module_or_test_filter> --report-time
```

For broad CLI behavior changes, use the `testing` skill to choose integration
or component prerequisites.
