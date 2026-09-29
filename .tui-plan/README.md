# Golem CLI TUI Plan

This directory tracks the active product, design, architecture, implementation,
and validation plan for the terminal UI in cli/golem-cli, exposed through both
golem tui and golem-cli tui.

## Current State

- The accepted visual foundation and the live typed Agents Overview are in
  production.
- Production currently exposes only Ops, Agents Overview, and a visibly fake
  Metrics interaction fixture.
- Dormant Dev, Server, REPL, Output, and older workspace code is implementation
  history, not the current product contract.
- The next rebuild restores Dev and expands Ops using the locked model in
  product-contract.md.
- Metrics acquisition, persistence, Docker integration, and a possible web UI
  are intentionally deferred until the provider-independent TUI feels right.

## Locked Direction

- The global context is app/env/server.
- Dev and Ops are the initial top-level workspaces, with navigation designed for
  additional workspaces later.
- Dev contains Build & Deploy, Server, and REPL as one primary view plus
  optional right or bottom subviews.
- Ops uses subject then view navigation with Agents, Deployment, Environment,
  and Server as minimal root subjects.
- Activity provides Timeline, Journal, and single-Agent Live representations.
- Metrics uses metric-family then concrete-series drill-down and keeps Golem
  service metrics distinct from Agent OTLP telemetry.
- Relevant resource mutations are available through contextual actions with
  explicit risk classes and a session-only remote write lock.
- Finite commands and mutations use the global Jobs overlay. Server, REPL, and
  Live are persistent sessions.
- Only visual preferences persist across restarts; operational state remains
  session-local.

## Sources Of Truth

- product-contract.md: locked behavior, vocabulary, scope, and remaining
  product decisions.
- ui-system.md: accepted visual and interaction rules.
- architecture.md: runtime boundaries, state ownership, providers, Jobs, and
  persistence structure.
- tasks.md: exact readiness checklist and executable backlog.
- phases.md: checkpoint order and acceptance gates.
- testing.md: test layers, scenarios, preview coverage, and validation commands.
- dx-ux-review.md: visual review ledger and active acceptance focus.
- progress.md: chronological implementation and validation history.

When documents disagree, product-contract.md owns product behavior,
ui-system.md owns rendering, and tasks.md owns current executable status.
Progress entries remain historical and must not override the active contract.

## Working Rule

Before implementing a milestone, close its unchecked planning gate in tasks.md,
update the owning source-of-truth document, and record a checkpoint-sized
implementation batch. After implementation, run the smallest focused tests,
the relevant preview coverage, package checks, and git diff --check before
marking the milestone complete.
