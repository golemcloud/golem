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

## Phase 3 Follow-Ups: Theme And Shortcuts

- [x] Research current non-TUI CLI decorators and color conventions.
- [x] Research opencode TUI theme/keybinding conventions.
- [x] Research golem.cloud visual tokens and colors.
- [x] Add Golem-branded default dark TUI theme tokens.
- [x] Preserve nested CLI and REPL ANSI colors instead of remapping them into the TUI theme.
- [x] Show tab numbering in the tab strip.
- [x] Make `r` the direct REPL start/focus shortcut.
- [x] Add general `Ctrl-X` leader mode for settings and secondary actions.
- [x] Move flag/settings toggles behind `Ctrl-X` leader shortcuts.
- [x] Use unified shortcut styling in footer, palette, and leader hints.
- [x] Apply manual visual polish for rails, Agents sidebar separator, output backgrounds, and segmented header labels.
- [x] Apply follow-up polish for single-background header, palette inner side rail, server toggle UX, and tab shortcut highlighting.
- [x] Show idle/running indicators for Output, Server, and REPL in the tab row.
- [x] Add centered Braille dashboard background logo generated from the in-repo Golem logo asset.
- [x] Widen and darken the Braille dashboard logo and remove command palette box borders.
- [ ] Manually review the updated Golem-branded TUI in the playground.

## Phase 4: Data Providers

- [x] Replace placeholder Agent tab with fuzzy-filtered agent list.
- [x] Add agent mode toggle: durable / ephemeral / all.
- [x] Add selected agent details side panel.
- [x] Add manual and auto-refresh for agents.
- [-] Remove placeholder Environment and Component tabs for now.
- [ ] Read profiles from config.
- [ ] Read manifest environments.
- [ ] Resolve selected environment.
- [ ] List visible environments directly or via nested CLI after evaluation.
- [ ] List components directly or via nested CLI after evaluation.
- [ ] List agents directly or via nested CLI after evaluation.
- [ ] Implement empty/error/loading states.
- [x] Add Agents inspect subview with oplog and stream split panes.
- [x] Add independent oplog/stream scrolling and pane focus switching.
- [x] Stop agent inspect jobs on Esc and return to the agent list.

## Phase 5: Nested CLI Jobs

- [x] Add local server tab backed by `golem server run`.
- [x] Capture local server logs with scrollback and autofollow.
- [x] Add start/stop/restart/clean-restart server actions.
- [x] Keep server logs separate from finite command output.
- [x] Add subtle section backgrounds and prefix glyphs after minimal layout review.
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

- [x] Add reusable PTY input encoder for interactive sessions.
- [x] Add terminal-screen session renderer for REPL.
- [x] Add action to launch REPL for selected environment.
- [x] Start with nested PTY-backed `golem repl`.
- [x] Decide whether first UX is embedded pane or managed full-screen child mode.
- [x] Add REPL focus and exit behavior.
- [ ] Manually verify embedded REPL behavior in the playground TUI.

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
