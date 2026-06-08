# Testing

The TUI needs tests that make interactive behavior visible and reproducible for autonomous development.

## Goals

- Verify terminal setup and cleanup.
- Verify key handling and state transitions.
- Verify rendered frames for important states.
- Verify nested CLI sessions can start, receive input, produce output, and exit.
- Provide readable frame dumps when tests fail.
- Keep tests purposeful and lightweight; do not add broad or brittle coverage just because the UI is new.
- Build agentic loops from the start, so implementation bugs can be diagnosed from captured frames, state dumps, and small commands.

## Test Layers

Testing should optimize for fast iteration and human reviewability. Prefer a few high-signal tests around the skeleton, render output, and nested process handling over a large test matrix.

### Pure State Tests

Test reducers and action handling without terminal rendering.

Examples:

- tab switching
- command palette filtering
- focus changes
- environment selection
- theme capability fallback

### Render Tests

Use `ratatui::backend::TestBackend` to render deterministic frames.

Compare frames through existing golden-file style tests where practical.

Frames should be normalized for dimensions and dynamic values.

### TUI Driver Tests

Create a `TuiTestDriver` that can:

- start from a constructed `TuiApp`
- send synthetic key events
- send resize events
- advance ticks
- capture frames as text
- assert selected state and visible text

This is the main framework for seeing what the TUI does without running a real terminal.

### PTY Integration Tests

Extend the existing `expectrl` interactive test style for end-to-end smoke tests.

Use these sparingly because they are slower and more platform-sensitive.

Initial PTY tests:

- `golem-cli tui` starts and renders a known title/status text
- `q` exits cleanly
- command palette opens from `Ctrl-P`
- a nested non-interactive CLI command can run and show output
- a REPL session can be launched or focused when that feature lands

## Frame Capture

On render or driver test failure, print or save the last rendered frame.

Frame captures should include:

- terminal size
- active view
- focus target
- selected environment
- visible text buffer

This is how an agent can inspect UI failures without visual terminal access.

Frame capture is part of the development workflow, not just CI. It should be easy to run a small command and inspect the latest rendered state.

## Agentic Development Loop

The initial scaffold should include a minimal loop for autonomous iteration:

- run a targeted compile/check command
- run a small render or driver test
- inspect a deterministic frame dump when behavior is wrong
- run a PTY smoke test only when terminal lifecycle or nested input/output changes

This loop should stay cheap enough to run repeatedly during implementation.

## Determinism

Tests should avoid live network calls unless explicitly marked integration-level.

Use fake data providers for render tests and state tests.

Use nested CLI test doubles where possible before invoking real `golem-cli`.

## Cross-Platform Notes

- PTY behavior differs across platforms; keep assertions broad in PTY tests.
- Prefer text presence and exit behavior over exact escape-sequence snapshots for PTY tests.
- Keep exact frame snapshots in `TestBackend` tests, not real terminal tests.

## Existing Assets To Reuse

- `expectrl` is already used by CLI interactive tests.
- `goldenfile` is already available in dev dependencies.
- `portable-pty` is already used by the CLI for REPL supervision.
- Current REPL tests include synchronization hooks that can inform later TUI REPL tests.
