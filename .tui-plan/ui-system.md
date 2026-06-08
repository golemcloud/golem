# UI System

The TUI should feel like a modern terminal application: keyboard-first, searchable, responsive, and usable without memorizing commands.

## Core Layout

The default screen should have:

- top context bar with selected profile, application, environment, and server kind
- tab strip for major views
- main content area
- optional side panel for details or command results
- bottom status bar with shortcuts and background activity

## Views

Initial views:

- Dashboard: selected context, health summary, recent tasks, quick actions.
- Environments: manifest environments, visible remote environments, selected target.
- Components: deployed components for the selected environment.
- Agents: agent list, filters, selected agent details.
- Commands: searchable command/action catalog.
- REPL: embedded or nested `golem repl` session.
- Output: logs and nested command output buffers.

Views should be cheap to add and should not own terminal lifecycle directly.

## Navigation

Initial key model:

- `q`: quit when no modal input is active
- `Esc`: close modal, blur input, or cancel current transient action
- `Tab` / `Shift-Tab`: cycle focus or tabs depending on context
- `[` / `]`: previous or next tab
- `Ctrl-P`: open command palette
- `:`: open command palette in command mode
- `/`: search or filter current view
- `Enter`: activate selected item
- `?`: show shortcuts/help

Exact shortcuts can change as the UI becomes concrete. Searchable commands are the stable discovery mechanism; shortcuts are accelerators.

## Command Palette

The palette should combine:

- built-in TUI actions
- selected CLI commands from `CliCommandMetadata`
- context-aware actions from the current view
- recently used commands

Each command should include:

- display label
- optional shortcut
- category
- description
- availability state
- execution mode: internal action, direct API, nested CLI PTY, nested CLI piped

Unavailable commands should be visible when useful, with a short reason.

## Multi-Environment Workflows

The UI should allow the user to keep multiple environment-bound workflows visible or resumable.

This favors nested CLI jobs because each job can carry explicit environment flags and its own output buffer.

The selected environment in the context bar controls default actions, but a job should retain the environment it was started with.

## Themes

Themes should be centralized and semantic. Widgets should ask for semantic styles rather than hard-coded colors.

Initial semantic roles:

- background
- foreground
- muted
- border
- selected
- focused
- success
- warning
- error
- info
- accent
- command

Initial themes:

- default dark
- high contrast
- monochrome fallback

## Color Capability

Detect terminal color capability and choose graceful defaults.

Inputs to consider:

- `NO_COLOR`
- `TERM=dumb`
- `COLORTERM=truecolor` or `COLORTERM=24bit`
- `TERM` values containing `256color`
- Windows terminal behavior through `crossterm`
- explicit future theme/config override

Capability levels:

- no color
- ANSI 16 color
- ANSI 256 color
- truecolor

The TUI should remain readable at every level.

## Accessibility And Usability

- Do not rely on color alone for status.
- Keep selected/focused states visually distinct in monochrome.
- Avoid flicker during refreshes.
- Preserve terminal state on exit.
- Provide explicit confirmation for destructive actions.
