# Golem CLI TUI Plan

This directory tracks the plan, decisions, task breakdown, and progress for adding a cross-platform terminal UI to `golem-cli`, exposed as `golem tui` and `golem-cli tui`.

The TUI should help users monitor and manage Golem environments without leaving the terminal. It should support switching between environments, running multiple workflows in parallel, launching and embedding the Golem REPL, and discovering commands through a searchable command palette.

## Product Goals

- Provide a modern TUI experience for day-to-day Golem development and operations.
- Work on macOS, Linux, and Windows.
- Treat local, cloud, and custom environments uniformly through environment selection.
- Make commands discoverable through search, while still supporting direct shortcuts.
- Support multiple views or tabs so users can monitor and act without tmux.
- Support theme selection and graceful color fallback based on terminal capabilities.
- Include a test framework that makes rendered interactive screens inspectable and reproducible.

## Initial Technical Direction

- Implement in `cli/golem-cli`, so both `golem-cli tui` and the recommended `golem tui` entry point work.
- Use `ratatui` for rendering and `crossterm` for terminal control and events.
- Prefer nested CLI sessions for early workflow integration when that is the fastest path to useful behavior.
- Choose direct API calls per task when they are simple, stable, and avoid unnecessary output parsing.
- Use existing command metadata and fuzzy matching for command discovery.
- Reuse existing PTY and interactive-test dependencies where possible.

## Key Decisions

- The TUI is not `golem`-only. It belongs in `golem-cli` and is exposed through both binaries.
- Environment switching is a first-class TUI concept. Local and remote should be handled by the same workflow wherever possible.
- Nested CLI execution is acceptable, especially early, because it preserves existing behavior and allows multiple environment-bound workflows.
- Direct API integration is decided case by case.
- The REPL starts as a managed embedded or nested interactive workflow, with deeper internal integration deferred until needed.
- Code should stay simple, reviewable, and low on moving parts. Avoid abstractions, framework layers, and tests that do not directly improve confidence or iteration speed.
- From the first scaffold, prioritize agentic coding loops: deterministic render output, compact smoke checks, and failure artifacts that make bugs inspectable without manual terminal interaction.

## Document Index

- `architecture.md`: runtime, module boundaries, data flow, and integration strategy.
- `ui-system.md`: visual system, views, navigation, palette, shortcuts, and themes.
- `testing.md`: render tests, PTY tests, frame capture, and manual inspection workflow.
- `phases.md`: milestone plan and acceptance criteria.
- `tasks.md`: actionable task backlog.
- `progress.md`: running implementation notes and status.
