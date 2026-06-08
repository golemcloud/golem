# Progress

## 2026-06-08

Initial planning context captured.

Decisions recorded:

- Implement the TUI in `cli/golem-cli` so both `golem tui` and `golem-cli tui` work.
- Prefer nested CLI sessions early where they preserve behavior and enable multi-environment workflows.
- Choose direct API integration case by case.
- Treat local, cloud, and custom targets as environment selections rather than separate UI modes.
- Use `.tui-plan` markdown files to track design, tasks, and progress.
- Keep implementation simple and optimized for human reviewability.
- Avoid needless tests and abstractions.
- Build agentic coding loops from the beginning with deterministic render/frame inspection and small targeted checks.

Repository observations:

- `golem-cli` already depends on `crossterm`, `portable-pty`, `expectrl`, `fuzzy-matcher`, and `goldenfile`.
- `ratatui` is not yet in workspace dependencies.
- Existing REPL implementation goes through `ReplHandler` and `TypeScriptRepl`, with PTY supervision for interactive mode.
- Existing CLI interactive tests use `expectrl` and can inform TUI PTY tests.
- Existing command metadata collection can be reused for command discovery.

Current status:

- Planning documents have been created.
- No code implementation has started yet.
