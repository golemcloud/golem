# TUI Design System

This is the authoritative design-system document for the Golem TUI. It defines
the target interaction model before more feature work continues.

The document is intentionally strict. Future TUI changes must either follow
these rules or update this document first.

## Core Model

The TUI is a keyboard-first operational shell for both local development and
production operations. Local development often includes monitoring, inspection,
and REPL-driven testing; production work may also use REPL-like inspection and
interactive operations. The shell must therefore avoid a hard split where Dev is
only local and Ops is only production.

The top-level workspaces are:

- Home;
- Dev;
- Ops.

The TUI must not add top-level workspaces for Jobs, Observe, Context, Settings,
Server, Output, or REPL. Those are global surfaces, panels, sub-pages, or jobs
inside the three workspaces.

Terms:

- Workspace: a top-level area. The initial workspaces are Home, Dev, and Ops.
- Sub-page or tab: a stable page inside a workspace.
- Panel: a named layout region inside a preset layout.
- Focus: the active panel or control that receives keyboard input.
- Job: a running or completed command, server process, REPL, stream, or
  inspection with output history and launch context.
- Context: the global selected target used by future actions. A context is not
  just an environment name; it is a resolved app/environment scope plus a
  server target, with enough selector provenance to explain why it is
  available.

## Global Shell

The header must show the selected context clearly. Context includes enough
target information for the user to understand where future actions will run.

Context switching must be a global picker or modal opened from the header,
palette, or shortcut. Context switching must not be a top-level workspace.

Settings must be reachable through palette, modal, or scoped actions. Settings
must not be a top-level workspace unless this document is revised.

The command palette is the universal jump surface. It should be able to search
and open:

- actions;
- workspaces and workspace sub-pages;
- entities and resource types;
- recent views;
- running or completed jobs;
- contexts and settings;
- layout targets;
- help topics.

The palette must not become the only way to use important workflows. Common
workflows still need visible workspace affordances and contextual help.

Unavailable actions should stay visible where the user expects them, but they
must be disabled and explain the reason.

Confirmations must use a consistent inline modal. Confirmation modals must
state the action, the affected jobs or resources, and the confirm/cancel keys.

## Workspaces

Home is the overview and jump workspace. It should combine:

- current context and health summary;
- project/application dashboard information;
- recent and running work;
- common commands;
- jump points into Dev and Ops.

Home should help users answer "where am I, what is running, and where should I
go next?" It should not become a dumping ground for every entity table.

Dev is the integrated workbench for building, running, testing, and inspecting
an application. Dev owns:

- build, deploy, and clean commands;
- local server controls and server logs as one view onto the global local
  server service;
- REPL;
- command output;
- app-local agents and components;
- pinned jobs and inspection panels relevant to the active development loop.

Dev must use preset layouts first. Arbitrary user-created splits are out of
scope until the preset model is proven. Presets should make the common loops
fast: command output beside logs, REPL beside server state, and app entities
beside details or streams.

The first Dev layout presets are right, left, top, and bottom. These presets
are session state only. Pointer resizing may adjust split ratios for the
current TUI run, but it must not write configuration or create a persistence
migration.

The global local server is not a top-level workspace. Its v1 primary full
surface is a toggleable right-side drawer available from Home, Dev, and Ops.
Dev may keep a compact Server panel when the drawer is closed, but when the
drawer is open the drawer is the primary server surface and Dev should avoid a
second large server log panel.

Ops is the operations and resource workspace. Ops opens on curated dashboard
tabs, not on a raw entity list. Dashboard tabs should be grouped by lifecycle
and user intent, not mechanically by CLI command names.

The initial Ops dashboard group names are placeholders:

- Dev;
- Deploy;
- ClickOps;
- Monitor.

These names and group boundaries are open questions and must be reviewed again.
The design intent is:

- basic development resources, especially agents and components;
- deployment, deployed entities, and environments;
- click-ops/admin entities such as secrets, policies, resources, accounts,
  tokens, plugins, and profiles;
- devops monitoring, including future server metrics and exposed operational
  telemetry.

Each Ops dashboard tab should drill into scoped explorer/detail views. The
generic explorer behavior lives inside the relevant dashboard scope rather than
as a separate top-level workspace.

Agents and components are shared entities between Dev and Ops. The UI must not
invent separate Dev-agent and Ops-agent concepts. Dev may show app-local
shortcuts and workflow-specific panels, but details should link to the same
underlying entity model used by Ops.

## Layout And Navigation

Top-level navigation switches only between Home, Dev, and Ops.

Top-level workspace jumps should use stable number keys:

- `1` for Home;
- `2` for Dev;
- `3` for Ops.

Next/previous workspace cycling should remain available. The exact shortcut can
be decided during implementation, but it must not require punctuation-heavy keys
as the only path because those are awkward on some keyboard layouts.

Workspace tabs mean stable sub-pages. They are not user-opened documents or
browser-style resource tabs.

Workspace sub-page and dashboard tab navigation should use leader commands by
default:

- `ctrl+x n` for next sub-page;
- `ctrl+x p` for previous sub-page.

Splits are named layout regions in a preset layout. A split must have a clear
panel identity and focus behavior.

Focus movement must be keyboard-first. The UI should provide:

- `tab` as the simple focus cycle inside the current layout;
- direct focus shortcuts for important panels;
- contextual help that names the focused panel and reachable focus targets.

Pointer interaction should become native to the layout model, while remaining
optional for primary workflows. Scrolling must affect the panel under the
pointer, clicks should focus tabs, splits, buttons, and selectable rows, and
split handles should be resizable by pointer once split geometry is stable.

Breadcrumbs should be used inside Ops drilldowns and other nested resource
views. Breadcrumbs are for orientation and jumping back up the current resource
path; they are not a replacement for top-level workspace navigation.

Breadcrumb segments should be keyboard-addressable through leader commands,
search, or another explicit keyboard path. They must not be visual-only
navigation.

`esc` steps out. It closes transient modes and modals first, leaves explicit
focus modes next, then exits the current drilldown one level. Normal shell
`esc` should not be the primary quit command.

`q` quits from a normal shell state. If jobs are running or state may be lost,
quit must show a confirmation modal.

The initial navigation history model is back/step-out only. Forward history is
deferred until there is a concrete workflow that needs it.

## Interaction And Shortcuts

Shortcut notation must be lowercase. Documentation should use forms such as
`ctrl+x`, `ctrl+x n`, `ctrl+p`, `esc`, `enter`, and `tab`. It must not use
capital letters in a way that implies `shift` unless `shift` is explicitly part
of the shortcut.

Only a few shortcuts should be truly global:

- workspace navigation;
- palette;
- help;
- quit;
- context switching when implemented.

Workflow actions should be scoped to their workspace, panel, or focused entity.
Build, deploy, clean, server control, and REPL control should move behind
leader commands rather than bare global keys.

The leader key is `ctrl+x`. Pressing it should open a visible transient menu.
The leader menu is the universal structured command grammar, while the palette
is universal search and jump.

Leader commands should be organized by mnemonic groups. Initial groups should
cover:

- workspace navigation;
- panel and focus movement;
- run commands;
- layout selection;
- context switching;
- Ops/entity actions;
- help.

Leader hints should show immediate available keys. Scoped help should provide
the full reference for the current workspace, sub-page, panel, and focus.

Text input owns printable keys. When a filter, search box, command input, or
other text field is focused, printable keys must type into that field. Only
escape, control keys, leader commands, and explicit accept/cancel keys should
interrupt text entry.

REPL raw-input mode must be explicit. Starting or showing a REPL panel should
not silently capture all normal TUI keys unless the user focuses the REPL input
mode. Leaving REPL focus returns keyboard input to the TUI.

Lists and tables should use arrow-first navigation:

- `up` and `down` move selection;
- `pageup` and `pagedown` move by page;
- `home` and `end` jump to boundaries;
- `enter` opens or drills into details.

`enter` on a row opens details. The leader menu exposes actions for the
selected row or entity. Destructive or mutating row actions should not rely on
bare direct keys as the only path.

Shortcut resolution is scope-first:

- focused input or panel;
- workspace or sub-page;
- global shell.

Help must show the active meaning of a reused shortcut. Avoid reuse where it
would be surprising, but reuse is allowed when focus makes the meaning clear.

## Jobs And Output

Output is not a top-level workspace. Output belongs to jobs.

Every command, server process, REPL, stream, and inspection should be modeled as
a job when it has lifecycle or output history. Jobs should record:

- kind;
- status;
- launch context;
- command or request summary;
- output or terminal screen;
- exit or stop state when available.

The Dev workspace should expose a job timeline and allow active jobs to be
pinned into panels. The same job can be reachable from Home summaries, Dev
panels, palette search, and contextual links without becoming a separate
top-level workspace.

Running jobs must keep their launch context identity visible in history and
details.

When the user switches global context while context-bound jobs are running, the
TUI must ask for confirmation. The confirmation must explain which jobs need to
stop, restart, or remain attached to their launch context. Server, build,
deploy, REPL, stream, and inspection jobs are context-bound unless documented
otherwise.

## Context And Environments

Local and production targets use the same workspace model for now. The selected
context controls action availability and resource data. Future customization may
adjust layout or actions by target type, but the base model must stay shared.

The TUI context model has two axes:

- app/environment scope: the application and environment that operations are
  scoped to when one is selected;
- server target: the Golem server/client/auth source used to query or mutate
  resources.

A manifested app context is selected when the current workspace has an
application manifest and the selected environment is one of the environments
defined by that manifest. This enables both Dev and Ops, because build, deploy,
clean, REPL, local-server control, and app-local resource shortcuts have a
manifest-backed application model.

A server target is selected from the union of:

- servers referenced by manifest environments, including omitted servers that
  imply the built-in local server;
- built-in local and cloud servers;
- servers from configured profiles;
- the server/profile/environment selectors implied by launch flags and their
  environment-variable overrides.

Server targets may overlap by endpoint. The TUI must not silently collapse
overlapping sources, because source and auth semantics matter. The picker may
group duplicates visually, but it must preserve selectable source variants such
as manifest server, built-in server, profile, and launch selector.

After selecting a server target, the user may select an application environment
known to that server. This server app-environment context is Ops-only by
default. It enables Dev only when it matches the current manifest application
and environment. Matching means the selected server app/environment identifies
the same application and environment as the manifest context, not merely a
similar display name.

Dev eligibility is therefore:

- enabled for manifested app contexts;
- enabled for server app-environment contexts only when they match the current
  manifest app/environment;
- disabled for server-only, profile-only, and unrelated app-environment
  contexts.

Ops eligibility is broader. Ops may run against any selected server target or
server app-environment context that has enough information for the requested
operation.

Local server is a global TUI-managed service, not a selected-context job. It
may be shown inside Dev and later as a global side panel, but changing the
selected context or leaving Dev must not stop it. Server launch uses launch
scope, not the currently selected Ops context. Selecting a built-in local server
target may query the running local server; if no local server is reachable, the
picker should report that as a loading error and must not auto-start it.

The selected-context model must account for all global selector inputs that can
change available options or context semantics:

- `--environment`, including `<env>`, `<app>/<env>`, and
  `<account>/<app>/<env>` forms;
- `--local` and `--cloud`;
- `--profile`;
- `--app-manifest-path` and `--disable-app-manifest-discovery`;
- `--preset`;
- `--config-dir`;
- `--dev-mode`;
- `--yes`, `--show-secrets`, auth token overrides, HTTP batch/parallelism
  tuning, server no-limit changes, offline/wasmtime cache flags, and other
  non-target flags when they affect generated actions, direct clients, or
  explanations;
- environment overrides such as `GOLEM_ENVIRONMENT`, `GOLEM_PROFILE`,
  `GOLEM_APP_MANIFEST_PATH`, `GOLEM_DISABLE_APP_MANIFEST_DISCOVERY`,
  `GOLEM_PRESET`, `GOLEM_AUTH_TOKEN`, and HTTP/server tuning variables after
  CLI resolution has applied them.

The TUI should keep a launch selector snapshot that records the resolved global
flags and the user-visible source of each selected axis. It should use that
snapshot to explain why candidates exist and to materialize future contexts or
direct clients.

`GolemCliGlobalFlags` is not a sufficient long-term internal representation for
every selected TUI target. In particular, "use this manifest-defined custom
server, then select this remote app environment" is a valid TUI target but does
not map cleanly to today's global flags unless the server is also available as
a profile or selected manifest environment. Future implementation should add a
TUI target descriptor that can materialize either an `Arc<Context>` or direct
clients without forcing every target through nested CLI flags.

The selected context applies to future actions only. Existing jobs keep their
launch context unless the user confirms a stop or restart.

## Actions And Help

Actions are the source of truth for user-visible commands. Each action should
define:

- stable ID;
- long label;
- shortcut when useful;
- category or workspace;
- scope;
- availability rule and reason;
- execution kind.

Palette rows, footer hints, leader hints, and help should derive from action
metadata where practical.

Help is scoped by workspace, sub-page, mode, and focus:

- global controls are always available;
- workspace controls appear only in the relevant workspace;
- panel and focus controls appear only when reachable;
- leader controls appear under the leader section;
- raw controls such as typing, scrolling, and selection may remain explicit
  help rows until they become actions.

Shortcut and action documentation must not drift from registered actions. The
later DX/UX hardening goal should define the source-of-truth and drift checks
for action docs, palette metadata, footer hints, and tests.

## Ops Data Rules

Ops and resource exploration should use direct typed calls by default.
Refresh-heavy Ops views must not parse nested CLI output.

Nested CLI remains acceptable for interactive Dev workflows where PTY behavior
matters, especially build, deploy, clean, server, and REPL flows.

Nested CLI in Ops views is transitional debt. Each remaining use must be
tracked until it has a direct typed provider or direct streaming provider.

Direct Ops calls should run through the context executor. Views own
request-specific decisions, local filtering, stale result handling, details,
and event mapping.

Command handlers should expose neutral data-returning helpers where needed.
They must not grow TUI-specific methods such as `for_tui`.

## Current Implementation Audit

Current top-level views are Dashboard, Agents, Output, Server, and REPL. These
do not match the target top-level model. Dashboard maps roughly to Home; Agents
maps to part of Ops and part of Dev; Output, Server, and REPL should become Dev
panels and jobs.

Current modes include normal, leader, palette, help, agent filter, command
interaction, REPL, and REPL leader. These are input/focus states, not
workspaces.

Current registered actions cover build, deploy, clean, server control, agent
refresh/settings, view navigation, REPL control, context switching, palette,
help, and quit.

Current global bare run keys such as `b`, `d`, `c`, `s`, and `r` are scaffolding
and should not be treated as final design. Current `tab` behavior cycles views,
which conflicts with the target focus model. Current numeric jumps should be
retained conceptually but remapped to Home, Dev, and Ops when workspaces land.

Agents list refresh already uses a direct typed context-executor call.

Agent inspect oplog and stream still use nested CLI. That is transitional Ops
debt and should move to direct streaming providers.

## Review Status

This document is accepted as the planning baseline for the Home, Dev, and Ops
workspace shell remap.

Accepted sections:

- Core Model;
- Global Shell;
- Workspaces;
- Layout And Navigation;
- Interaction And Shortcuts;
- Jobs And Output;
- Context And Environments;
- Actions And Help;
- Ops Data Rules;
- Current Implementation Audit;
- Roadmap;
- Accessibility And Stability.

The remaining open questions are non-blocking deferred decisions. They should
be answered when their owning milestone is planned or when implementation hits
the decision directly. They must not block the first workspace-shell remap.

## Roadmap

Milestone 0: review and accept this design-system document. This is complete.
Major workspace or navigation reshaping may now be planned from this baseline.

Milestone 1: align interaction metadata with the design-system vocabulary. Fix
shortcut notation, action metadata drift, execution kind drift, availability
reasons, and help wording. This milestone should be behavior-preserving except
for clearer unavailable-action handling and help/metadata text. This is
complete.

Milestone 2: implement the Home, Dev, and Ops workspace shell full remap.
Dashboard should become Home. Output, Server, and REPL should become Dev panels
and jobs. Agents should move into the new workspace model while preserving the
existing list, filter, details, refresh, and inspect behavior during the
transition. This milestone may reuse the initial TUI context.

Milestone 3: harden global selected-context UX. Replace the current flat
context picker with a scoped target model that separates server targets from
app/environment scopes, derives options from all resolved global selectors, and
gates Dev/Ops availability by the rules in this document. Add confirmation for
context switches that affect running jobs.

Milestone 4: migrate remaining Ops refresh and inspect paths to direct typed or
streaming providers. Agent oplog/stream is the first known streaming debt.

Milestone 5: split stable modules out of `app.rs` after the rules settle.
Likely modules include actions, help, selected context, jobs, layouts, and Ops
view state.

Milestone 6: plan local observability integration. Local/server metrics should
fit into Dev panels and Ops Monitor dashboard surfaces, not a new top-level
workspace.

## Open Questions

These questions are deferred and non-blocking:

- Final Ops dashboard group names and boundaries. Use the placeholder groups
  for planning, then rename or regroup during Ops dashboard implementation.
- Which Dev layout presets should ship first. Start from the existing Output,
  Server, REPL, and Agents workflows during the workspace-shell remap, then
  refine presets once the panels exist in the new model.
- Whether Home should eventually support configurable dashboard sections. Keep
  the first Home static and curated.
- How much customization local and production contexts should get after the
  shared model is implemented. Keep the shared model first; revisit during the
  selected-context UX milestone.
- Which action/help documentation artifacts should be generated during DX/UX
  hardening. Defer until the action and workspace model has settled.

## Accessibility And Stability

The UI must not rely on color alone for running, selected, disabled, or error
state.

Labels should remain stable and should not cause layout reflow when toggles or
status indicators change.

Nested CLI output and REPL terminal screens should preserve ANSI output. The
Golem TUI theme should style the surrounding chrome, not rewrite interactive
program output.
