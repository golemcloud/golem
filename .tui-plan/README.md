# Golem CLI TUI Plan

This directory is the source of truth for the terminal UI exposed through
`golem tui` and `golem-cli tui`.

## Current State

- The shared visual foundation and the Dev workspace are implemented.
- Dev owns the nested-CLI application loop: Plan, Build, Deploy, Clean, Server,
  and REPL.
- Ops implements the stable Overview, Agents, Oplog, Stream, Metrics,
  Deployments, and Configuration destinations, with typed routes and explicit
  scope handoffs.
- Local observability, bounded oplog search, the grouped journal, and the
  minimal-Grafana metrics views are implemented and awaiting combined terminal
  acceptance rather than further prototype compatibility work.
- Native SSH is an Agent-scoped Ops route backed by the same session engine as
  the standalone Reedline command. Exporter publication remains a separate
  follow-up; the current released exporter stays authoritative until then.

## Locked Direction

- The global context remains `app/env/server`.
- Dev and Ops remain the initial workspaces.
- Ops destinations are stable, horizontal tabs:
  `Overview · Agents · Oplog · Stream · Metrics · Deployments · Configuration`.
- Ops is observe/investigate-first and read-only in this milestone.
- Metrics is local-first and TUI-only, with curated Dashboard and ad-hoc
  Explore modes. Grafana and a web UI are deferred.
- Oplog is both a readable grouped journal and an exact indexed forensic view.
- Configuration has Environment, Account, and Server scopes.
- Only visual preferences persist across restarts; operational state remains
  session-local.

## Sources of Truth

- `product-contract.md`: product behavior and scope.
- `ui-system.md`: rendering and interaction rules.
- `architecture.md`: runtime, state, provider, and protocol boundaries.
- `tasks.md`: executable checklist.
- `phases.md`: checkpoint and commit order.
- `testing.md`: automated and manual acceptance matrix.
- `dx-ux-review.md`: current visual review ledger.
- `progress.md`: chronological implementation history.

When documents disagree, product contract owns behavior, UI system owns
presentation, architecture owns boundaries, and tasks owns current status.
