# Tasks

This is the authoritative TUI backlog. Keep it lightweight, reviewable, and synchronized with `.tui-plan/progress.md`.

## Status Markers

- `[ ]` planned
- `[~]` in progress
- `[x]` done
- `[!]` blocked
- `[-]` deferred or dropped

Rules:

- Keep at most one task `[~]` while actively implementing.
- Mark a task `[x]` only after code and relevant validation are done.
- If a task grows, split it instead of keeping a vague checkbox.
- Record important decisions, validation commands, and direction changes in `progress.md`.
- Record any added abstraction with the reason it is needed.
- After TUI implementation changes, prepare the matching manual playground by reinstalling `golem` for the active checkout.

## Phase 1: Scaffold

- [x] Add `ratatui` to root `[workspace.dependencies]`.
- [x] Add `ratatui = { workspace = true }` to `cli/golem-cli/Cargo.toml`.
- [x] Add `Tui` to `GolemCliSubcommand`.
- [x] Add command help text and examples for `tui`.
- [x] Add `TuiCommandHandler` and dispatch from `CommandHandler::handle_command`.
- [x] Ensure `golem tui --help` works from the installed `golem` binary.
- [x] Implement terminal guard.
- [x] Implement event-driven terminal loop.
- [x] Redraw only after input or resize in the initial scaffold.
- [x] Avoid fixed polling or global frame ticks.
- [x] Implement clean shutdown for normal exit.
- [x] Define minimal `TuiApp` state.
- [x] Define selected context display model.
- [x] Implement dashboard layout.
- [x] Implement status/footer bar.
- [x] Add `TestBackend` render test.
- [x] Add readable frame dump helper for render test assertions.
- [x] Add a cheap targeted command/check agents can run after TUI edits: `cargo check -p golem-cli && cargo test -p golem-cli renders_dashboard_frame`.

## Phase 1 Follow-Ups

- [ ] Add panic/error cleanup strategy for terminal restoration.
- [x] Add a small manual smoke note or script for launching the installed playground TUI.
- [ ] Decide whether a terminal parser such as `vt100` is needed for PTY frame tests, or whether `expectrl` plus text assertions is enough initially.
- [ ] Verify raw mode and alternate screen manually on macOS after each terminal-lifecycle change.

## Phase 2: Navigation And Actions

- [x] Implement initial view navigation and command palette shell.
- [x] Define `TuiView` or tab enum.
- [x] Define focus model.
- [ ] Define notification/status model.
- [x] Implement tab strip.
- [x] Implement current-view help/shortcut overlay.
- [x] Define `TuiAction` registry.
- [ ] Add command availability reasons.
- [x] Add direct internal action execution mode.

## Phase 3: Command Palette

- [x] Implement command palette modal.
- [x] Add command palette filtering with `fuzzy-matcher`.
- [x] Add built-in TUI actions to the palette.
- [ ] Add filtered CLI command metadata to the palette.
- [x] Add palette interaction test.

## Phase 4: Data Providers

- [ ] Read profiles from config.
- [ ] Read manifest environments.
- [ ] Resolve selected environment.
- [ ] List visible environments directly or via nested CLI after evaluation.
- [ ] List components directly or via nested CLI after evaluation.
- [ ] List agents directly or via nested CLI after evaluation.
- [ ] Implement empty/error/loading states.

## Phase 5: Nested CLI Jobs

- [x] Modernize header/body/output borders.
- [x] Hide command input row when it is not relevant.
- [x] Stabilize Output view layout and reduce command/footer reflow.
- [x] Add command status spinner while nested commands are running.
- [x] Run build/deploy through PTY-backed nested CLI commands.
- [x] Add command interaction mode.
- [x] Implement nested process model.
- [-] Implement piped command output capture.
- [x] Implement PTY command output capture.
- [x] Route input to focused PTY job.
- [x] Preserve per-job command options and environment overrides.
- [x] Redraw from nested CLI output events, not polling.
- [x] Preserve colored output in Output view.
- [x] Auto-follow output and allow scrollback.
- [x] Add key and mouse output scrolling with scrollbar.
- [x] Add native cursor placement for command interaction input.
- [x] Compact command status to one line.
- [x] Keep yes/reset flag state visible in the footer.
- [x] Support Ctrl-C/Esc cancel and force-kill escalation.
- [x] Add build/deploy option toggles.
- [x] Add clean nested CLI command.
- [x] Improve command shortcut and flag hints.
- [x] Fix output scroll-top viewport behavior.
- [ ] Replace line-oriented PTY output with terminal-screen emulation for inline cursor rendering.

## Phase 6: REPL

- [ ] Add action to launch REPL for selected environment.
- [ ] Start with nested PTY-backed `golem repl`.
- [ ] Decide whether first UX is embedded pane or managed full-screen child mode.
- [ ] Add REPL focus and exit behavior.

## Phase 7: Agentic Testing Loop

- [ ] Add pure state tests where they improve reviewability.
- [ ] Add `TuiTestDriver`.
- [ ] Add frame dump on driver failure.
- [ ] Add PTY smoke test for start and quit.
- [ ] Avoid large snapshot suites until the UI stabilizes.

## Cross-Platform

- [ ] Keep PTY assumptions isolated for Windows compatibility.
- [ ] Avoid Unix-only terminal APIs in core TUI runtime.
- [ ] Gate platform-specific code behind small modules if needed.
