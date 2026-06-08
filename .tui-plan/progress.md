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

Navigation and palette scaffold:

- Added static views: Dashboard, Environments, Components, Agents, and Output.
- Added tab rendering and view switching with `]`, `[`, `Tab`, `Shift-Tab`, and `1` through `5`.
- Added a command palette opened with `Ctrl-P` or `:`.
- Added fuzzy filtering with `fuzzy-matcher` over built-in TUI actions.
- Added built-in palette actions for view switching and quitting.
- Added keyboard routing so palette input is handled separately while the palette is open.
- Added focused tests for active tab rendering, tab switching, numeric jumps, palette opening, palette filtering, and palette action execution.
- Workflow decision: after TUI implementation changes, prepare the matching manual playground by reinstalling `golem` for the active checkout. For this checkout, use `CARGO_INSTALL_ROOT=/Users/noise64/.cargo-alt-02 cargo make install-golem-dev-release`, then test from `/Users/noise64/workspace/golem-demo/golem-02` with `/Users/noise64/.cargo-alt-02/bin` first in `PATH`.
- Fixed tab label jitter by rendering inactive tabs with the same width as active bracketed tabs.
- Added `TuiMode` for normal, palette, and help modal routing.
- Added `?` help overlay with current global and palette shortcuts.
- Added `Show Help` as a built-in command palette action.
- Added focused tests for opening help, closing help, and opening help from the palette.

Build/deploy nested CLI feature:

- Added a channel-driven TUI event loop so terminal input and nested command output are both event sources.
- Added a PTY-backed nested CLI runner for finite commands.
- Added build/deploy shortcuts: `b` and `d`.
- Added `--yes` and `--reset` toggles: `y` and `r`.
- Added CommandInteraction mode for build/deploy prompts and finite command input.
- Added Ctrl-C/Esc cancellation with second press force-kill escalation.
- Added Output view rendering for command status, flags, command line, and scrollable output.
- Added auto-following output with PageUp/PageDown/Home/End scroll controls.
- Added ANSI color preservation with `ansi-to-tui`, with plain fallback on parse failure.
- Added command palette actions for Build, Deploy, Toggle Yes, and Toggle Reset.
- Added focused tests for toggles, command state, output follow/scroll behavior, and cancel escalation.
- Added native cursor placement for command interaction input, using the terminal cursor rather than drawing a fake cursor.
- Made `yes` and `reset` flag state visible in the global footer.
- Compacted the Output command summary to a single status line.
- Enabled mouse capture and mouse-wheel scrolling in the Output view.
- Added a vertical scrollbar for command output.
- Added focused tests for footer flags, compact status, command-interaction cursor placement, arrow-key scrolling, and mouse scrolling.
- Added `clean` as a PTY-backed nested CLI command with `c` shortcut and palette action.
- Styled footer and compact Output status hints so shortcut letters and enabled flags stand out.
- Fixed top-of-output scrolling so the viewport stays filled instead of collapsing to a single line plus empty space.
- Fixed scrollbar positioning so the thumb reaches the bottom when viewing the latest output.
- Added focused tests for clean shortcut, clean palette action, and top-scroll viewport filling.
- Added a focused test for output scrollbar top/bottom position mapping.
- Stabilized the Output view layout by always reserving the command input row.
- Removed active-tab brackets and switched active tab indication to styling only, avoiding tab label width changes.
- Reworked the footer into fixed-width command and flag segments to avoid reflow when toggling `yes` or `reset`.
- Reworked compact command status into fixed-width cells for command kind, status, flags, command, and hint.
- Kept the main boxes for this pass; visual simplification can continue after manual review.
- Added focused tests for fixed-width labels and stable Output input-row reservation.
- Added a per-command spinner event source that runs only while a nested command is active.
- Rendered the spinner in the compact command status row for running/cancelling commands.
- Stopped spinner ticks when commands finish, fail to start, are killed, or are cleaned up on TUI exit.
- Added a focused spinner tick test.
- Modernized the main TUI chrome by removing boxes from the header, non-output body, and Output sections while keeping modal boxes.
- Added a subtle horizontal separator under the tab row.
- Hid the command input row when no command is running or when the current command was started with `yes:on`.
- Preserved the command input row for the lifetime of commands started with `yes:off`, even if the future-run `yes` toggle changes during the run.
- Added focused tests for hidden/visible command input row behavior.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- RustRover build check for `cli/golem-cli/src/tui/app.rs`
- RustRover build check for `cli/golem-cli/src/tui/mod.rs` and `cli/golem-cli/src/tui/nested_cli.rs`
- RustRover build check for `cli/golem-cli/src/tui/terminal.rs`
- RustRover build check for `cli/golem-cli/src/tui/app.rs` after clean/scroll polish

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- RustRover build check for `cli/golem-cli/src/tui/app.rs`
