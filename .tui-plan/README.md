# Golem CLI TUI Plan

This directory tracks the active product and implementation plan for the terminal UI in `cli/golem-cli`, exposed as `golem tui` and `golem-cli tui`.

The TUI is being rebuilt from the accepted terminal design foundation. The
first production surface is an Ops-first agent explorer backed by the existing
typed agent-list call. Earlier Home, Dev, server-drawer, REPL, command-output,
and nested agent-inspect UI remains implementation history rather than the
visible product contract.

## Current Direction

The current work promotes the accepted visual foundation into production and
uses it to establish the first real product workflow:

- A rule-based action system for long names, shortcuts, palette entries, help, footer hints, scopes, and availability.
- A test driver and high-level scenario tests that reduce regressions between manual TUI reviews.
- A TUI-owned context model for manifest, local, cloud, custom, and non-manifest modes.
- A scoped logging context so concurrent direct handler calls can capture logs without global cross-talk.
- An Ops-only shell that starts on the Agents subject and retains `[Ops]` as
  the single workspace selector without reserving a keyboard-layout-dependent
  number shortcut.
- Overview and Metrics as views over the selected server/app/environment scope;
  Metrics currently contains a permanently labelled fake OTLP explorer for UI
  review, while Activity follows only with a direct typed provider.
- Reusable cursor-backed collections with exact server Dataset filters, local
  loaded-row Find, explicit loaded/more-available state, and focusable
  resizable details.
- A local observability implementation for GOL-162, continuing the discovery
  recorded in [golemcloud/golem#3456](https://github.com/golemcloud/golem/issues/3456).

## Main Arcs

1. Action and help rules.
2. Test driver and regression harness.
3. Scoped logging and context execution.
4. Context and environment model.
5. Ops-first production rebuild.
6. Local observability workflow for GOL-162.

## Key Decisions

- Keep the TUI in `golem-cli` so both entry points use the same implementation.
- Prefer nested CLI/PTTY workflows where they preserve existing dev/interactive behavior; ops/resource exploration should move toward direct typed calls.
- Treat the initial CLI `Context` as input, not permanent global truth. The TUI needs its own selected context and each job needs an immutable launch context.
- Do not centralize all TUI event mapping in a generic data provider. Use a small context executor that handles context/logging/background execution while each view owns its request and event mapping.
- Do not add TUI-specific command handler methods such as `list_agents_for_tui`. Extract or expose neutral data-returning helpers and let CLI rendering plus TUI mapping sit at the edges.
- Derive visible shortcuts and help from action metadata wherever possible.
- Keep neutral context, terminal, PTY, job, and output building blocks, while
  deleting or hiding obsolete workspace-specific UI instead of preserving a
  compatibility renderer.
- Keep tests focused on behavior and inspectable frame output instead of large brittle snapshots.

## Document Index

- `architecture.md`: runtime, action rules, context model, job model, and module direction.
- `ui-system.md`: visual system, action/help rules, shortcuts, and dev/ops workspace split.
- `testing.md`: TUI driver strategy, scenario coverage, and validation commands.
- `dx-ux-review.md`: case-by-case design review workflow, coverage, and active
  feedback ledger.
- `phases.md`: prioritized arcs and acceptance criteria.
- `tasks.md`: active goal-sized backlog.
- `progress.md`: running implementation notes and validation history.
