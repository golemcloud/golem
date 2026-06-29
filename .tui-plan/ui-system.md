# UI System

The TUI should feel consistent: command names, shortcut keys, palette rows, help, and footer hints should agree because they are derived from the same rules.

## Layout

- Header: selected application/context/environment/server information.
- Tabs: Dashboard, Agents, Output, Server, REPL, with running indicators where relevant.
- Body: active workflow or resource view.
- Footer: high-value global actions and state flags.
- Modal overlays: command palette and help.

## Action Rules

Every user command should have one long name and, where useful, one shortcut.

Action metadata should define:

- stable ID;
- long label;
- shortcut;
- category/workspace;
- scope;
- availability;
- execution kind.

Visible surfaces should use those definitions:

- palette shows searchable long names and shortcuts;
- footer shows the most important global actions;
- leader hint shows leader-scoped settings/actions;
- help groups actions by scope and adds raw input controls where needed.

## Context Help Rules

Help is scoped by current mode, view, and focus:

- global controls are always available;
- leader controls appear under the leader section;
- view-specific controls appear only for the relevant view or subview;
- focused workflows, such as REPL and agent inspect, must have reachable help;
- raw controls such as typing, scrolling, and selection can be explicit help controls until they become actions.

## Dev And Ops Workspaces

Dev workflows:

- build;
- deploy;
- clean;
- server management;
- REPL;
- manifest exploration.

Ops workflows:

- agent list and inspect;
- components and resources;
- logs and streams;
- future local observability and metrics.

The command palette can bridge both, but footer/context help should stay focused on the current workspace.

Ops/resource views should eventually use direct typed calls through a context executor. The executor handles context/logging/background mechanics, while the view keeps control over modes, local filtering, selection, details, and event mapping.

## Context And Environment UX

Future environment switching should distinguish:

- manifest environment;
- local explicit mode;
- cloud explicit mode;
- custom named environment;
- non-manifest/config-only mode.

The header should show the selected context clearly. Jobs must show the context they launched with.

## Accessibility

- Do not rely on color alone for running/selected/error state.
- Keep labels stable and avoid layout reflow when toggles change.
- Preserve nested CLI and REPL ANSI output; theme only the surrounding chrome.
