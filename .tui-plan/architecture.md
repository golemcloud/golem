# Architecture

The TUI should be a thin interactive shell over existing Golem behavior at first. It should not duplicate the CLI command stack unless direct integration is clearly better for a specific task.

## Entry Point

Add a top-level `Tui` variant to `GolemCliSubcommand` in `cli/golem-cli/src/command.rs`.

Dispatch through `CommandHandler` to a new `TuiCommandHandler` in `cli/golem-cli/src/command_handler/tui.rs`.

The handler creates the existing `Context`, then starts the TUI runtime.

## Module Layout

Planned source layout:

```text
cli/golem-cli/src/tui/
  mod.rs
  app.rs
  action.rs
  command_palette.rs
  data.rs
  event.rs
  terminal.rs
  theme.rs
  widgets.rs
  repl.rs
  nested_cli.rs
  views/
    mod.rs
    dashboard.rs
    environments.rs
    components.rs
    agents.rs
    repl.rs
```

## Runtime Model

The runtime owns the terminal and drives a loop with these inputs:

- terminal key and mouse events
- terminal resize events
- background task results
- nested CLI session output

The runtime updates `TuiApp` state, then renders a full frame.

Keep the runtime small. Prefer one straightforward event loop and plain state structs over callback-heavy abstractions or generic framework layers.

The runtime should be event-oriented. It should block waiting for terminal input when there is no background work, and redraw only after input, resize, nested CLI output, background task completion, or explicit scheduled refresh events. Avoid a fixed polling render loop.

When periodic refresh is needed, model it as an explicit event source for the view or job that needs it, not as a global frame tick.

## Terminal Lifecycle

Terminal setup should be guarded so raw mode, alternate screen, cursor visibility, and mouse/focus modes are restored after normal exit, errors, and panics where possible.

The existing worker watch mode already has alternate-screen cleanup patterns, but the TUI should own a separate reusable guard.

## State Model

`TuiApp` should contain:

- selected environment and profile context
- active tab or view
- command palette state
- focus target
- theme and color capability
- status notifications
- nested CLI sessions
- last known data snapshots per view
- refresh state and pending task IDs

State should be easy to inspect in tests and debug dumps. Avoid hiding important behavior behind trait objects unless there is a concrete need.

## Environment Model

Environment selection should be independent of whether the target is local, cloud, or custom.

Each view/action should receive an explicit environment context instead of assuming the process-level selected environment forever.

For nested CLI execution, this means commands should be spawned with explicit flags when needed, such as `--environment`, `--local`, `--cloud`, `--config-dir`, and app manifest flags.

## Integration Strategy

Prefer nested CLI sessions initially when they provide immediate reuse of existing workflows:

- commands with complex output or prompts
- commands that already manage deployment/build/repl behavior
- parallel workflows against different environments
- commands where behavior stability matters more than structured data

Prefer direct API calls when they are simple and avoid fragile output parsing:

- listing visible environments
- loading local profiles and config
- reading manifest environment definitions
- polling health/status endpoints
- fetching structured component or agent lists when existing client calls are already straightforward

Expose data-returning helpers from existing command handlers only when needed and keep current CLI output behavior unchanged.

When both a direct call and a nested CLI command are viable, choose the option that produces the smallest understandable change for the current task. Revisit only when the simpler choice blocks usability or testing.

## Nested CLI Sessions

Nested CLI sessions should be represented as stateful jobs with:

- command line and working directory
- environment variables
- PTY or piped process mode
- output buffer
- current lifecycle state
- focused input routing status
- exit status and error summary

PTY mode is required for interactive children such as `golem repl` and commands with prompts. Piped mode is enough for non-interactive commands whose output is rendered into an output pane.

## REPL Integration

The first REPL implementation can be nested and PTY-backed. Input is routed to the REPL only when the REPL pane is focused.

Later, if needed, shared REPL internals can be extracted from `ReplHandler` and `TypeScriptRepl`, but that should not block the initial TUI.

## Server Commands

The TUI lives in `golem-cli`, which may compile without `server-commands`. Local server management actions should therefore be feature-aware.

When running through `golem`, the existing `server-commands` feature is enabled and local server actions can call or nest `golem server ...` workflows.
