# Golem CLI TUI Plan

This directory tracks the active product and implementation plan for the terminal UI in `cli/golem-cli`, exposed as `golem tui` and `golem-cli tui`.

The TUI is no longer just a scaffold. It currently has Dashboard, Agents, Output, Server, and REPL views; PTY-backed build/deploy/clean, server, REPL, and agent inspect workflows; a command palette; a Golem-branded dark theme; and focused render/state tests.

## Current Direction

The next work should invest in foundations that make larger UX and feature goals safe:

- A rule-based action system for long names, shortcuts, palette entries, help, footer hints, scopes, and availability.
- A test driver and high-level scenario tests that reduce regressions between manual TUI reviews.
- A TUI-owned context model for manifest, local, cloud, custom, and non-manifest modes.
- A scoped logging context so concurrent direct handler calls can capture logs without global cross-talk.
- A clear split between dev workflows and ops/resource exploration.
- A future observability home for [golemcloud/golem#3456](https://github.com/golemcloud/golem/issues/3456), initially as a TUI launcher/status/control surface unless the POC proves the TUI should own more.

## Main Arcs

1. Action and help rules.
2. Test driver and regression harness.
3. Scoped logging and context execution.
4. Context and environment model.
5. Dev/ops workspace structure.
6. Local observability workflow for issue #3456.

## Key Decisions

- Keep the TUI in `golem-cli` so both entry points use the same implementation.
- Prefer nested CLI/PTTY workflows where they preserve existing dev/interactive behavior; ops/resource exploration should move toward direct typed calls.
- Treat the initial CLI `Context` as input, not permanent global truth. The TUI needs its own selected context and each job needs an immutable launch context.
- Do not centralize all TUI event mapping in a generic data provider. Use a small context executor that handles context/logging/background execution while each view owns its request and event mapping.
- Do not add TUI-specific command handler methods such as `list_agents_for_tui`. Extract or expose neutral data-returning helpers and let CLI rendering plus TUI mapping sit at the edges.
- Derive visible shortcuts and help from action metadata wherever possible.
- Grow architecture through small stable seams, not a broad rewrite of `app.rs`.
- Keep tests focused on behavior and inspectable frame output instead of large brittle snapshots.

## Document Index

- `architecture.md`: runtime, action rules, context model, job model, and module direction.
- `ui-system.md`: visual system, action/help rules, shortcuts, and dev/ops workspace split.
- `testing.md`: TUI driver strategy, scenario coverage, and validation commands.
- `dx-ux-review.md`: story-by-story design review workflow, coverage, and active
  feedback ledger.
- `phases.md`: prioritized arcs and acceptance criteria.
- `tasks.md`: active goal-sized backlog.
- `progress.md`: running implementation notes and validation history.
