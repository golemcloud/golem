# Tasks

This is the initial backlog. Items can be split or reordered as we refine the plan.

## Documentation

- Keep `.tui-plan/progress.md` updated after each implementation step.
- Record direct API vs nested CLI decisions as they are made.
- Add sketches or frame dumps for important UI states once rendering exists.
- Record any added abstraction with the reason it is needed.

## Dependencies

- Add `ratatui` to root `[workspace.dependencies]`.
- Add `ratatui = { workspace = true }` to `cli/golem-cli/Cargo.toml`.
- Decide whether a terminal parser such as `vt100` is needed for PTY frame tests, or whether `expectrl` plus text assertions is enough initially.

## Command Wiring

- Add `Tui` to `GolemCliSubcommand`.
- Add command help text and examples for `tui`.
- Add `TuiCommandHandler` and dispatch from `CommandHandler::handle_command`.
- Ensure `golem tui --help` and `golem-cli tui --help` work.

## Runtime

- Implement terminal guard.
- Implement event loop.
- Implement tick handling.
- Implement clean shutdown.
- Add panic/error cleanup strategy.
- Keep the first runtime implementation in a small number of files until real pressure appears to split it further.

## App State

- Define `TuiApp`.
- Define `TuiView` or tab enum.
- Define focus model.
- Define notification/status model.
- Define selected environment model.

## Rendering

- Implement dashboard layout.
- Implement status bar.
- Implement tab strip.
- Implement command palette modal.
- Implement empty/error/loading states.

## Commands And Actions

- Define `TuiAction` registry.
- Add command palette filtering with `fuzzy-matcher`.
- Add command availability reasons.
- Add nested CLI execution mode.
- Add direct internal action execution mode.

## Data Providers

- Read profiles from config.
- Read manifest environments.
- Resolve selected environment.
- List visible environments directly or via nested CLI after evaluation.
- List components directly or via nested CLI after evaluation.
- List agents directly or via nested CLI after evaluation.

## Nested CLI

- Implement nested process model.
- Implement piped command output capture.
- Implement PTY command output capture.
- Route input to focused PTY job.
- Preserve per-job environment flags.

## REPL

- Add action to launch REPL for selected environment.
- Start with nested PTY-backed `golem repl`.
- Decide whether first UX is embedded pane or managed full-screen child mode.
- Add REPL focus and exit behavior.

## Testing

- Add pure state tests.
- Add `TestBackend` render tests.
- Add `TuiTestDriver`.
- Add frame dump on failure.
- Add PTY smoke test for start and quit.
- Add palette interaction test.
- Add a cheap targeted command/check that agents can run after each TUI edit.
- Avoid large snapshot suites until the UI stabilizes.

## Cross-Platform

- Verify raw mode and alternate screen on macOS.
- Keep PTY assumptions isolated for Windows compatibility.
- Avoid Unix-only terminal APIs in core TUI runtime.
- Gate platform-specific code behind small modules if needed.
