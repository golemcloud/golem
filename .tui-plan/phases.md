# Phases

This plan keeps the first implementation useful while leaving room for richer integration.

## Phase 0: Planning And Skeleton

Acceptance criteria:

- `.tui-plan` documents exist and are kept up to date.
- `ratatui` dependency plan is agreed.
- Initial module layout is agreed.
- First milestone scope is explicit.

## Phase 1: Minimal TUI Runtime

Acceptance criteria:

- `golem tui` and `golem-cli tui` parse successfully.
- TUI opens in alternate screen and exits with `q`.
- Terminal state is restored on exit and error.
- A dashboard frame renders selected context information.
- Headless render test covers the dashboard.
- A small local check command exists for agentic iteration on the TUI skeleton.
- A failed render test provides enough frame output to diagnose the visible state.

## Phase 2: Navigation And Command Palette

Acceptance criteria:

- Tabs or views can be switched from keyboard.
- Command palette opens and filters actions.
- Built-in TUI actions can be executed from the palette.
- CLI command metadata is available to the palette with a TUI-specific filter.
- Driver tests cover palette open, search, select, and cancel.
- Tests remain focused on behavior that is difficult to review manually from code.

## Phase 3: Environment-Aware Data Views

Acceptance criteria:

- Environment view shows manifest and/or visible environments.
- Selected environment can be changed from the TUI.
- Component or agent view can load data for the selected environment.
- Direct API vs nested CLI integration is documented per view.
- Tests cover environment switching and empty/error states.

## Phase 4: Nested CLI Jobs

Acceptance criteria:

- TUI can start a nested non-interactive CLI command.
- Command output appears in a pane or output view.
- Multiple jobs can be tracked in state.
- Jobs retain their launch environment.
- Failed commands show status without corrupting terminal state.

## Phase 5: REPL Workflow

Acceptance criteria:

- TUI can launch `golem repl` from a selected environment/context.
- REPL output is visible in the TUI or managed full-screen child mode.
- Keyboard input is routed predictably while REPL is focused.
- REPL exit returns to the TUI cleanly.
- PTY smoke test covers launch and exit.

## Phase 6: Themes And Capability Fallback

Acceptance criteria:

- Theme system uses semantic styles.
- Color capability is detected.
- Monochrome fallback is readable.
- Theme tests cover no-color, ANSI, 256-color, and truecolor decisions.

## Phase 7: Management Workflows

Acceptance criteria:

- Common actions can be started from views and palette.
- Destructive actions require confirmation.
- Local server actions are feature-aware.
- Cloud/custom environment workflows do not require separate UI paths.
