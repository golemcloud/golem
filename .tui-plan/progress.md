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
- `ratatui` was not in workspace dependencies before this scaffold.
- Existing REPL implementation goes through `ReplHandler` and `TypeScriptRepl`, with PTY supervision for interactive mode.
- Existing CLI interactive tests use `expectrl` and can inform TUI PTY tests.
- Existing command metadata collection can be reused for command discovery.

Current status:

- Planning documents have been created.
- TUI scaffolding has started with dependency wiring, command dispatch, a minimal terminal runtime, and a dashboard render test.
- Added `ratatui 0.30.1` as a workspace dependency and wired it into `golem-cli`.
- Added the top-level `tui` command for both `golem-cli tui` and `golem tui` through existing command dispatch.
- Excluded `tui` from REPL command metadata because it is an interactive top-level mode, not a REPL command candidate.
- Added a minimal TUI runtime using `ratatui` plus `crossterm`, with raw mode, alternate screen, cursor hide/show, and cleanup through a terminal guard.
- Switched the scaffold runtime from fixed polling to blocking terminal events; redraws now happen after keypresses and resizes.
- Added an initial dashboard render with selected application/environment/server/config context.
- Added a focused `test-r` render test that captures the `TestBackend` buffer as readable text for agentic inspection.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli renders_dashboard_frame`
- RustRover build check for the touched TUI and command wiring files
