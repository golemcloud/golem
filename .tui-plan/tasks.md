# Tasks

This is the active TUI backlog. Keep it goal-sized and synchronized with `progress.md`.

## Status Markers

- `[ ]` planned
- `[~]` in progress
- `[x]` done
- `[!]` blocked
- `[-]` deferred or dropped

## Completed Goal: Action And Help Rules

- [x] Add action IDs, categories, scopes, execution kind, and palette visibility.
- [x] Derive palette filtering from visible actions and include category/scope/execution text in search.
- [x] Derive footer, leader hints, and help from registered action metadata.
- [x] Keep raw input controls as explicit context-help controls.
- [x] Make Agent inspect help reachable.
- [x] Add focused action/help drift tests and a small test driver.

## Current Goal: Scoped Logging And Context Executor Foundation

- [x] Introduce a scoped `LogContext` that can capture output without changing global logging for unrelated work.
- [x] Make `LogIndent` and `LogOutput` restore the context they were created against, including across `.await`.
- [x] Add async capture and spawn helpers that propagate logging context and tracing span.
- [x] Add focused logging tests for two concurrent captures, indentation across `.await`, scoped output mode, and global fallback behavior.
- [ ] Replace the temporary provider shape with a smaller context executor that owns context generation, background execution, and logging scope only.
- [ ] Keep view-specific request building and `TuiEvent` mapping in the views or app state, not in the executor.
- [ ] Remove TUI-specific handler APIs and extract neutral data-returning helpers only when they are useful for both CLI and TUI call paths.

## Next Planned Goals

- [ ] Add action availability reasons and show unavailable actions consistently.
- [ ] Introduce selected TUI context and immutable job launch context.
- [ ] Move Agents list refresh from nested JSON CLI output to a direct typed call using the context executor and a neutral worker-list helper.
- [ ] Replace Agent inspect oplog/stream nested CLI with direct streaming providers.
- [ ] Split stable action/help/test-driver pieces out of `app.rs` after the rules settle.
- [ ] DX/UX hardening: design a lightweight documentation system for TUI actions, shortcuts, help surfaces, and interaction conventions after logging/context execution and direct ops refresh fundamentals are stable.
- [ ] Design environment switching and non-manifest modes.
- [ ] Define dev/ops workspace navigation and context-help behavior.
- [ ] Plan the TUI role for local observability from issue #3456.

## Deferred Goal: DX/UX Hardening

- [ ] Define the source-of-truth format for documenting actions, shortcuts, scopes, availability rules, and help text.
- [ ] Decide which user-facing surfaces are generated from that source: help, palette metadata, footer hints, docs, and test expectations.
- [ ] Add drift checks so shortcut/action documentation cannot silently diverge from the registered TUI actions.
- [ ] Review interaction conventions for leader actions, raw controls, focus modes, details panels, and refresh-heavy ops views.
- [ ] Keep this behind the current fundamentals: scoped logging/context execution, selected context handling, and the first direct typed ops provider.

## Ongoing Validation

- [ ] Keep `cargo check -p golem-cli` green after TUI changes.
- [ ] Keep `cargo test -p golem-cli --lib -- tui::` green after TUI changes.
- [ ] Manually smoke test installed TUI after terminal lifecycle, PTY, or large UX changes.
