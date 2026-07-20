# Progress

## 2026-07-02

TUI visual preview workflow:

- Added a dependency-free static HTML preview gallery under `.tui-plan/design-previews/`.
- The gallery compares four visual-language directions for headers, split affordances, color weight, density, server drawer treatment, palette styling, and Ops contrast.
- The preview is explicitly non-authoritative; accepted directions must be copied into `.tui-plan/ui-system.md`, then implemented in Ratatui and validated with render tests.

## 2026-07-01

Pointer-native layout and global server drawer:

- Added `tui::layout` as the first dedicated layout/hit-test module.
- Render now computes a named `LayoutSnapshot` and stores it on `TuiApp` for mouse handling.
- Mouse wheel routing now targets the panel under the pointer rather than the keyboard-focused panel.
- Pointer clicks now switch workspace tabs, focus Dev panels, select Ops agent rows, focus inspect panes, and activate context picker/confirmation regions.
- Added session-only split dragging for Dev primary/secondary splits and the global server drawer, with ratio clamps for constrained terminals.
- Added Dev layout presets for right, left, top, and bottom panel placement.
- Added `ctrl+x l` / palette action for cycling Dev layout presets.
- Added a global local-server drawer, closed by default and available from Home, Dev, and Ops.
- Added `ctrl+x v` / palette action for toggling the server drawer.
- When the drawer is open, Dev skips the large Server panel so server logs have one primary surface.
- Added focused layout and pointer behavior tests for preset geometry, drawer splitting, hit testing, region scrolling, clicks, modal actions, and split dragging.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Selected Context UX foundation:

- Added a global context picker opened with `ctrl+x e` and through the palette.
- Context candidates are built from the launch context, manifest environments, explicit local/cloud selections, and configured profiles.
- Switching builds a new immutable `Context` snapshot from cloned CLI flags and installs it into the TUI context executor with a new generation id.
- Context switching is blocked while context-bound work is active; confirmation/stop-and-restart flows are deferred.
- Nested dev/interactive commands inherit the selected context arguments, while direct Agents refresh uses the selected executor context.
- Command, server, REPL, agent inspect, and agent refresh status surfaces show their launch context.
- Local-server actions are disabled, and direct shortcuts are guarded, when the selected context is not local-server backed.

Selected Context UX scoped selector:

- Replaced the flat context candidate list with scoped context targets: manifest app contexts, server targets, and server app-environment targets.
- The context picker now uses a two-step flow. Manifest app contexts switch directly. Server targets first list visible app environments through a direct typed environment handler call, then the selected app/environment becomes the Ops scope.
- Server target discovery now includes manifest environment servers, built-in local/cloud servers, configured profiles, and launch-selector-implied targets while preserving source/auth variants as separate rows.
- Agents refresh carries the selected typed `EnvironmentReference` into the direct worker list request, so server app-environment scopes no longer rely on nested CLI parsing.
- Dev actions are enabled for manifest app contexts and disabled for Ops-only server app-environment contexts. Local server actions still additionally require a local-server-backed context.
- Agent inspect oplog/stream remain nested CLI transitional Ops debt until the direct streaming provider milestone.

Selected Context UX architecture correction:

- The first picker foundation is intentionally not the final context model.
- The design now separates manifest app contexts, server targets, and server app-environment contexts.
- Manifest app contexts enable Dev + Ops. Server app-environment contexts are Ops-only unless they match the current manifest app/environment.
- Server targets must be derived from manifest environment servers, built-in local/cloud servers, configured profiles, and all launch selector inputs, not only `--local` and `--cloud`.
- Overlapping server endpoints must preserve source/auth variants instead of being silently collapsed.
- The current `GolemCliGlobalFlags`-based candidate representation is documented as a temporary implementation gap; future work needs a TUI target descriptor that can materialize contexts or direct clients.

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
- Added subtle background colors for header, tabs, body surface, command status, and footer.
- Added section prefix glyphs (`┃`, `│`) to restore visual structure without returning to heavy boxed layouts.
- Standardized a left `┃` rail across header, tabs, separator bar, surfaces, command status, output, input, and footer.
- Replaced the horizontal dash separator with a full-width background bar row and a left rail.
- Changed the spinner first frame from `-` to `|` to better match the rail-based visual language.
- Added a Server tab backed by a separate PTY `golem server run` job.
- Added server log capture with independent scrollback and autofollow.
- Added server controls: `s` start/stop, `R` restart, `x` toggle clean, `C` clean restart.
- Added server palette actions for start, stop, restart, clean restart, toggle clean, and go to Server.
- Kept server logs separate from finite build/deploy/clean command output so both can coexist.
- Added focused tests for server initial rendering, clean toggles, start/stop/restart state, server log separation, and mouse scrolling.
- Made the command palette width content-driven, with terminal-margin clamping for narrow screens.
- Kept command palette width stable while filtering by sizing it from the full action catalog, not the filtered result set.
- Kept command palette height stable while filtering by sizing it from the maximum visible action count, not the filtered result set.
- Removed placeholder Environments and Components tabs for now.
- Replaced the Agents placeholder with a fuzzy-filtered agent list backed by `golem agent list --format json --mode <mode>`.
- Added agent mode cycle: durable, ephemeral, all.
- Added selected-agent details side panel with raw JSON fallback.
- Added manual agent refresh (`u`) and auto-refresh toggle (`a`).
- Added Agent filter mode entered with `/`, with editable query and Up/Down selection.
- Added agent palette actions for refresh, auto-refresh, mode cycle, and details toggle.
- Added robust JSON parsing for both array and `{ values: [...] }` agent list shapes.
- Added focused tests for tab removal, agent filtering/selection, mode cycling, details panel, refresh result handling, and JSON parsing.
- Added reusable PTY input encoder for future REPL mode and reused it for finite command interaction.
- Added PTY input encoding tests for printable, control, alt, and navigation keys.
- Added a `vt100`-backed terminal screen wrapper for future REPL rendering.
- Terminal screen wrapper can feed PTY bytes, resize, expose cursor position, render Ratatui lines, and retain plain text for assertions.
- Added terminal screen tests for plain text, cursor movement, cursor position, SGR styles, and resizing.
- Hid the command input row when no command is running or when the current command was started with `yes:on`.
- Preserved the command input row for the lifetime of commands started with `yes:off`, even if the future-run `yes` toggle changes during the run.
- Added focused tests for hidden/visible command input row behavior.
- Added a REPL tab backed by nested PTY `golem repl`.
- Added REPL-specific TUI events for PTY output, output closure, and process exit.
- Added `ReplState` and `ReplRun` with a `vt100` `TerminalScreen` instead of line-oriented output buffering.
- Added REPL view rendering with a compact status row, embedded terminal screen, and native cursor placement.
- Added REPL focus mode that sends keys directly to the REPL.
- Added `Ctrl-X` REPL leader commands: `q` leave focus, `k` stop, `p` palette, `?` help, `r` restart.
- Added `l` / Enter start-or-focus behavior and REPL palette actions for start/focus/leave/stop.
- Added focused tests for REPL start/rendering, terminal-screen output, cursor placement, and leader behavior.
- Researched non-TUI CLI styling and decorator conventions: green action words, yellow warnings, red errors, bold highlights, `-` bullets, UTF-8/ASCII comfy-table presets, `╔═`/`║`/`╚═` decorated help blocks, and `│` child-process output gutters.
- Researched opencode theme/keybinding conventions: semantic theme tokens, truecolor expectation, system/terminal theme mode, `none` terminal defaults, and `Ctrl-X` leader key use.
- Researched golem.cloud visual tokens: dark neutral surfaces, amber primary accent, warm orange marker, muted grey text scale, and dark code-block surfaces.
- Added an internal Golem-branded dark TUI theme token set; no custom/system theme support yet.
- Switched TUI chrome from cyan-led styling to Golem amber active/focus styling.
- Kept nested CLI output and REPL screen ANSI-native; the Golem theme only styles surrounding chrome, gutters, status rows, palette, help, and selection surfaces.
- Added tab numbering in the tab strip.
- Changed the direct REPL shortcut from `l` to `r`.
- Added a general `Ctrl-X` leader mode for settings and secondary actions.
- Moved `--yes`, `--reset`, server clean, server restart/clean-restart, and agent settings toggles behind `Ctrl-X` leader shortcuts.
- Added unified shortcut styling in footer, palette rows, and leader hints.
- Added `Restart REPL` to the command palette and bound focused REPL restart to `Ctrl-X R`.
- Added focused tests for numbered tabs and leader shortcut hint rendering.
- Polished the Golem-branded theme after manual review: removed redundant content/footer rails, made dashboard surface rails consistently gray, changed the Agents detail divider to gray, normalized output line backgrounds to the TUI surface, added segmented warm header backgrounds, and added spaces after header labels such as `app: `.
- Follow-up polish: restored gray content rails consistently on dashboard/content/output lines so non-empty rows no longer erase the surface rail while blank rows keep it.
- Fixed the dashboard rail rendering at the root cause: body content now reserves the first columns and the gray surface rail is drawn after body content, with a regression test asserting every dashboard body row keeps the rail. The header is now a full amber background with darker warm segment backgrounds for `app: `, `env: `, and `server: `.
- Header polish: restored the first-line `┃` separator, removed the multi-background gradient, and made the full header row amber with separate label/value text colors.
- Palette polish: added an inner `┃` side decoration to command palette content.
- Server UX polish: replaced separate `Start Server` and `Stop Server` palette entries with a single `Start/Stop Server` action using `s`, made `s` global so it jumps to Server and toggles the server, and added Server-tab `Enter` to toggle server like REPL-tab `Enter` starts/focuses REPL.
- Tab polish: tab numbers now use the same highlighted shortcut styling as footer/palette shortcuts.
- Added live tab indicators on the tab row for running Output commands, Server, and REPL sessions.
- Replaced the hand-written Braille dashboard mark with faint centered Braille art generated from the in-repo `website/src/assets/logo/golem-horizontal-white.png` logo asset.
- Changed tab running indicators to always render for Output, Server, and REPL: gray `○` when idle and green `●` when running.
- Widened the generated Braille logo by changing its aspect ratio, darkened it to a more background-like gray, and made dashboard text rows paint their full width so the logo does not bleed through after foreground text.
- Simplified the command palette by removing the old box border and changing the side rail to a full-height yellow rail for the whole palette subwindow.
- Added an Agents inspect subview within the Agents tab. Pressing Enter on a selected agent opens a split view with `golem agent oplog <agent>` on the left and `golem agent stream <agent>` on the right.
- Agent inspect mode uses independent PTY-backed nested CLI targets and output buffers for oplog and stream.
- Left/Right switches focused pane; Up/Down, PageUp/PageDown, Home/End scroll the focused pane; Esc kills both inspect jobs and returns to the agent list.
- Added focused tests for opening inspect mode, pane focus switching, focused-pane scrolling, Esc returning to the list, split-view rendering, and separate event routing for oplog/stream buffers.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- `cargo test -p golem-cli -- tui::app::tests tui::terminal_screen::tests`
- `cargo test -p golem-cli -- tui::`
- `cargo check -p golem-cli`
- `CARGO_INSTALL_ROOT=/Users/noise64/.cargo-alt-02 cargo make install-golem-dev-release`
- `PATH="/Users/noise64/.cargo-alt-02/bin:$PATH" golem tui --help` from `/Users/noise64/workspace/golem-demo/golem-02/test-app`
- RustRover build check for touched TUI files after theme/keymap pass
- RustRover build check for `cli/golem-cli/src/tui/app.rs`
- RustRover build check for `cli/golem-cli/src/tui/mod.rs` and `cli/golem-cli/src/tui/nested_cli.rs`
- RustRover build check for `cli/golem-cli/src/tui/terminal.rs`
- RustRover build check for `cli/golem-cli/src/tui/app.rs` after clean/scroll polish

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- RustRover build check for `cli/golem-cli/src/tui/app.rs`

## 2026-06-29

Planning reset and first foundation goal implemented.

Decisions recorded:

- Future TUI work is organized around six arcs: action/help rules, test driver/regression harness, scoped logging/context execution, context/environment model, dev/ops workspace structure, and local observability for issue #3456.
- The immediate implementation goal is "Action Rules And TUI Test Driver Foundation".
- The TUI should derive visible command names and shortcuts from action metadata where practical.
- Raw input controls such as text entry and scrolling can remain explicit help controls until they become actions.

Current status:

- Reworked `.tui-plan` documents around the current implementation state and foundation-first arcs.
- Extended `TuiAction` with stable IDs, categories, scopes, and palette visibility.
- Made palette filtering respect palette visibility and include action category text in fuzzy matching.
- Derived footer command labels/shortcuts and leader workflow shortcuts from the action registry.
- Replaced the static help body with generated sections backed by registered actions plus explicit raw input controls.
- Made Agent inspect help reachable with `?`.
- Added a test-only `TuiTestDriver` with key input, frame capture, visible/absent text assertions, and failure dumps.
- Added high-level driver scenarios for global action help, leader action help, palette/action-registry consistency, Agent inspect scoped help, and REPL leader help.
- The new driver initially caught that full-help assertions were invalid at 24 rows because the help modal clipped lower sections; the driver now uses a taller frame for full-help scenarios while existing 100x24 tests remain unchanged.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui::`

Direct Agents Provider Foundation was explored, then reverted.

Async TUI blocking-boundary cleanup:

- Replaced blocking terminal event reading with `crossterm::event::EventStream`.
- Kept `portable-pty` as the nested interactive backend, but isolated remaining blocking calls behind named PTY adapter helpers.
- Changed nested CLI runtime control operations so key handling, resize handling, and stop/kill actions queue bounded control messages instead of calling PTY write/resize/kill methods inline.
- Documented that `spawn_blocking` is allowed only for explicit blocking-library adapters, not normal TUI orchestration or direct ops/provider work.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Direct typed Agents refresh:

- Added a neutral `AgentListRequest` and `WorkerCommandHandler::list_agent_metadata` that returns `AgentsMetadataResponseView`.
- Kept CLI `agent list` rendering/logging at the CLI edge while sharing the typed listing helper.
- Changed the TUI Agents refresh to run through `TuiContextExecutor`, carrying generation plus launch context identity into `AgentRefreshFinished`.
- Mapped `AgentsMetadataResponseView` into `AgentListItem` directly and kept details backed by serialized typed metadata.
- Removed production JSON parsing and nested `golem agent list --format json` subprocess refresh from the Agents view.
- Made the context-executor reuse-id test order-independent because concurrent requests can complete in either order.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`
- `cargo test -p golem-cli --lib -- command_handler::worker --report-time`

Decisions recorded:

- Ops/resource exploration should use direct typed calls by default.
- Nested CLI remains appropriate for dev/interactive workflows where PTY behavior matters.
- Remaining ops nested CLI uses, including Agent inspect oplog/stream, are transitional debt until direct streaming providers exist.
- A central `TuiOpsProvider`/`TuiDataProvider` is the wrong shape if it absorbs every view's request and event mapping.
- The next direct-call foundation should be a smaller context executor: selected context ownership, launch context cloning, target generation, background async execution, logging scope, and completion delivery.
- Views should keep request-specific logic. For Agents, the view/app state should own mode selection, local mapping to rows, stale result handling, filtering, details, and events.
- Command handlers should not grow TUI-specific methods such as `list_agents_for_tui`. Extract or expose neutral data-returning helpers and keep CLI rendering at the CLI edge.
- Proper logging context is required before broad direct handler use. Thread-local logging alone is insufficient because existing `LogIndent` scopes commonly cross `.await` and TUI work may run concurrently.
- Logging context should be explicit and async-aware: a scoped `LogContext` owns output mode, indentation, buffers, and captured lines; lookup can use tracing span extensions and Tokio task-local scope before falling back to global CLI logging.
- Future environment switching should replace selected `Arc<Context>` values instead of mutating a live `Context`.

Current status:

- Reverted the premature provider implementation, including the central provider module, `WorkerCommandHandler::list_agents_for_tui`, and typed Agents refresh event wiring.
- Updated `.tui-plan` to make scoped logging and a context executor the next foundation before moving Agents list to direct typed calls.
- Kept the hard architecture rule: dev/interactive workflows may continue using nested CLI/PTTY, while ops/resource refresh-heavy views should move away from parsing nested CLI output.

Research notes:

- `Context` contains lazy clients, app context state, and caches; selected contexts should be treated as immutable launch inputs rather than mutable global targets.
- `Context::new` and environment/app resolution can have side effects such as manifest upgrades, app context initialization, component selection mutation, and server-side environment/app creation.
- Global or static state that matters for TUI concurrency includes CLI logging state, buffered logging, terminal width caching, program lookup caching, cargo target-dir caching, SDK override caching, and per-application agent type caches.
- Existing CLI logging uses global `LOG_STATE` and `LOG_STATE_BUFFER`; `LogIndent` and `LogOutput` mutate global state on construction/drop.
- Many command handlers hold logging scopes across `.await`, so a correct logging design must bind scopes to a context and preserve them across async execution.

Action And Help Rules completed.

Current status:

- Extended TUI action metadata with stable IDs, categories, scopes, execution kind, and palette visibility.
- Kept `TuiActionKind` as the local execution enum while using `TuiActionId` for UI derivation.
- Made palette search use palette-visible actions and include category, scope, and execution kind text.
- Derived footer shortcuts, normal leader hints, REPL leader hints, and help content from registered actions.
- Kept raw input controls such as scrolling, filtering, pane focus, and stdin routing as explicit context-help rows.
- Made `?` reachable from Agent inspect mode.
- Added a small test-only `TuiTestDriver` for key input and configurable frame assertions.
- Added focused tests for unique action IDs, palette visibility/category search, footer and leader drift, global help, Agent inspect help, REPL leader help, and command interaction help.

Validation:

- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Review follow-up:

- The action/help cleanup is enough for now, but a later DX/UX hardening goal should design a lightweight documentation system around TUI actions, shortcuts, help surfaces, and interaction conventions.
- That hardening work is intentionally not next; it should come after the fundamentals are stable, especially scoped logging/context execution and the first direct typed ops refresh path.

Scoped logging foundation implemented.

Current status:

- Replaced the single global CLI log state with a global fallback `LogContext` plus Tokio task-local scoped contexts.
- Added captured logging output for future direct TUI handler execution.
- Made `LogIndent` and `LogOutput` restore the context they were created against, including when dropped from another async scope.
- Added scoped `LogContext::scope`, `LogContext::spawn`, `buffered_lines`, and `take_buffered_lines`.
- Kept existing logging call sites working through the active context fallback.
- Left the context executor and direct Agents refresh for the next implementation slice.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- log --report-time`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Context executor foundation implemented.

Current status:

- Added a small concrete TUI context executor module for `Arc<Context>` with an initial launch context, stable context id, active Tokio runtime handle reuse, scoped log capture, and generic completion event mapping.
- Kept the TUI runtime path async: `tui::run` is awaited from command dispatch, events are delivered through bounded Tokio MPSC, and spinner/auto-refresh ticks run as Tokio tasks.
- Added bounded-event backpressure behavior: important events await or blocking-send into the queue, while spinner/auto-refresh ticks are best-effort and dropped when the queue is full.
- Switched the temporary nested Agents refresh subprocess from `spawn_blocking` plus `std::process::Command` to `tokio::process::Command`.
- Isolated truly blocking terminal and PTY reads to Tokio blocking workers while feeding the same async TUI event loop; TUI-owned raw PTY threads were removed.
- Factored nested CLI target-to-event mapping helpers with focused tests.
- Wired production TUI startup to create one executor from the initial `Arc<Context>` without creating a nested runtime.
- Removed the executor spawn counter and kept tests focused on delivered completion events.
- Kept request construction and `TuiEvent` mapping outside the executor so it does not become a central provider.
- Kept Agents refresh on the existing nested JSON CLI path for this slice.
- Added executor tests with real CLI contexts for launch context delivery, captured logs, separate concurrent log buffers, error delivery, and active runtime reuse.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- context_executor --report-time`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

## 2026-06-30

TUI design system discovery started.

Decisions recorded:

- Pause further implementation slicing until the TUI itself has a stronger design system.
- Do not write the initial design system document immediately.
- First run an interview with the user to settle the important product and interaction questions.
- Create the initial document only after the current implementation, desired dev workflow, desired ops explorer, navigation model, and layout rules are clear enough.
- Review the document with the user and repeat the interview/revision loop until the design system is accepted.

Current status:

- Updated the TUI backlog so the current goal is interview-driven design system discovery.
- Marked the existing `ui-system.md` as a lightweight snapshot, not the authoritative final design system.
- Kept implementation goals such as selected context, action availability, direct streaming providers, and module splitting behind the design-system review loop.
- Rewrote `ui-system.md` as the initial authoritative design-system document.
- Added interaction and shortcut rules after a second interview pass.
- Standardized shortcut notation on lowercase forms such as `ctrl+x`, `ctrl+p`,
  `esc`, `enter`, and `tab`; uppercase letters must not imply `shift` unless
  `shift` is explicitly part of the shortcut.
- Recorded the target interaction grammar: few globals, number-based workspace
  jumps, `ctrl+x` as a visible transient leader menu, leader-based run actions,
  `tab` for panel focus, `esc` for step-out, `q` for quit with confirmation,
  explicit REPL raw-input focus, text input ownership of printable keys, and
  arrow-first list/table navigation.
- Reordered the TUI backlog around the accepted design direction: design review
  remains blocking for major redesign; the next meaningful product slice is the
  Home/Dev/Ops workspace shell full remap; low-risk interaction metadata cleanup
  may happen before the shell; selected context UX follows the first shell
  remap; direct streaming providers, module splitting, and local observability
  follow afterward.

Interaction metadata cleanup implemented.

Current status:

- Standardized user-facing TUI shortcut notation to lowercase forms such as
  `ctrl+x`, `ctrl+p`, `esc`, `enter`, `tab`, and explicit `shift+...` when a
  shifted key is actually required.
- Marked typed Agents refresh as a direct action instead of nested CLI metadata.
- Added action availability results with reasons for finite commands while a
  command is running, Agents refresh while already running, and Agents refresh
  when the context executor is unavailable.
- Kept unavailable palette actions visible, muted, and annotated with the
  reason; unavailable palette actions do not execute.
- Updated help, leader hints, footer/status strings, palette rendering, and
  focused tests for the new notation and availability rules.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Design-system review pass completed.

Current status:

- Reviewed `ui-system.md` section by section and accepted it as the planning
  baseline for the Home, Dev, and Ops workspace shell remap.
- Recorded the accepted sections directly in the design-system document:
  core model, global shell, workspaces, layout/navigation,
  interaction/shortcuts, jobs/output, context/environments, actions/help, Ops
  data rules, implementation audit, roadmap, and accessibility/stability.
- Reclassified the remaining open questions as non-blocking deferred decisions.
  They should be resolved by the milestone that owns them, not before the first
  workspace-shell remap can be planned.
- Marked the design discovery goal and Milestone 0 review gate complete in the
  TUI backlog.
- Left Rust code unchanged for this pass.

Validation:

- `git diff --check -- .tui-plan/ui-system.md .tui-plan/tasks.md .tui-plan/progress.md`

Workspace shell full remap implemented.

Current status:

- Replaced the old Dashboard, Agents, Output, Server, and REPL top-level TUI
  views with Home, Dev, and Ops workspaces.
- Remapped number navigation to `1` Home, `2` Dev, and `3` Ops; `[` and `]`
  cycle workspaces, while `tab` cycles Dev panel focus.
- Added the first Dev workbench layout with REPL as the primary panel and
  Output, Server, and Agents as secondary panels, with focused-panel fallback
  rendering on narrow terminals.
- Routed build/deploy/clean to Dev Output, server actions to Dev Server, and
  REPL actions to Dev REPL while preserving existing nested CLI behavior.
- Remapped Ops to the existing full Agents explorer, preserving typed refresh,
  mode cycle, auto-refresh, fuzzy filtering, details, stale-result handling,
  errors, and inspect split behavior.
- Updated action metadata, palette, help, tabs, and tests to use
  workspace/panel vocabulary instead of old top-level view names.
- Kept the initial TUI context model unchanged; selected-context switching is
  still the next milestone.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Global local server service foundation implemented.

Current status:

- Reclassified local server as a launch-scoped TUI service instead of a
  selected-context dev job.
- Kept server controls and logs visible through Dev while allowing the service
  to survive workspace and selected-context changes.
- Removed local server from context-switch blockers and dev-stop confirmation.
- Kept build/deploy/clean and REPL as context-bound jobs that still require
  confirmation before a selected-context switch can stop them.
- Made server start/restart use launch-scoped CLI args rather than the current
  selected Ops context args.
- Added a follow-up pointer-native layout milestone for mouse-position-aware
  scrolling, clickable focus/selection, split resizing, layout presets, and
  server side-panel decisions.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`
