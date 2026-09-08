# TUI Design System

This is the authoritative design-system document for the Golem TUI. It records
accepted foundation rules and the current interaction model while the broader
product flows are reviewed.

The document is intentionally strict. Future TUI changes must either follow
these rules or update this document first.

## Real-renderer previews

TUI design work uses deterministic stories rendered by the actual Ratatui
widget tree. Run `cargo make tui-preview` for the watched browser gallery,
`cargo make tui-preview-terminal` for the watched interactive terminal gallery, or
`cargo make tui-preview-export` for a standalone artifact under `target/`.

The browser opens on one explicit current focus with Frame Base and no more than
three mutations. Its URL identifies the focus, view mode, font, and font size,
so feedback and follow-up changes should reuse copied gallery links. Production,
case browsing, and coverage live under collapsed secondary tools. Browser ligatures are disabled to
better match terminal cell rendering. Version-pinned Fira Code and
commit-pinned Iosevka Term files load directly from jsDelivr so a browser on a
different machine sees the same fonts. The gallery verifies regular and bold
cell widths plus joined vertical decorators before showing a frame; it blocks
review instead of silently falling back to a platform font. Coverage mode
renders every case under one selected variant. Track normalized
feedback and review status in `dx-ux-review.md`; durable rules accepted through
that review still belong in this document.

The `Production` visual style is the only runtime style. `Frame Base` is the
preview-only baseline for temporary current-focus mutations. They are
comparison material, not themes. Accepted rules are deliberately folded back
into Frame Base and all mutations are deleted before the next focus begins.
Explicit promotion into `Production` happens later.

### Programmatic design sources

A locked visual decision must have one executable representation in addition
to its written rule. Use a named widget, semantic token, typed rendering model,
or tested layout invariant according to the kind of decision. Preview stories
provide content and application state, but must not recreate accepted glyphs,
spacing, alignment, colors, selection treatment, or boundary behavior locally.

The preview component system owns context headers, pane boundaries, workspace
selectors, shortcut rows, scrollbars, popup frames and titles, search inputs,
selectable rows, decision-table rows, section headings, label/value fields,
status markers, notices, content tables, and output rows. `TuiVisualStyle` owns
their semantic colors. Explicit pane and footer geometry owns junction
placement. These components remain preview-only until Frame Base is deliberately
promoted into Production.

`PaneLayout` is the executable source for one-or-more-pane horizontal geometry.
The same pane weights determine header widths, body divider columns, and footer
junctions. It alone renders shared vertical dividers and top junctions; pane
content must not add a rail beside an internal divider. The terminal-facing
right edge remains title-only `┐` plus the workspace-row `┘`, with no body rail.
Body boundaries render after pane content so content cannot punch blank cells
through the outer left spine or shared dividers.

When a decision is revisited, change its existing representation and focused
tests. Temporary alternatives may exist only for the active comparison and are
removed when the new choice is locked.

Design-lab cases are synthetic, deterministic, offline render fixtures. They are useful
for visual comparison and render regression tests, but they do not exercise
providers or process lifecycles. Use `cargo make tui-preview-live -- <global
flags>` to launch the checkout-built production `golem <global flags> tui`
runtime. The terminal preview catches watcher termination and returns through
its raw-mode and alternate-screen cleanup guard before it is rebuilt. Neither
preview watcher supervises the live runtime.

## Core Model

The TUI is a keyboard-first operational shell for both local development and
production operations. Local development often includes monitoring, inspection,
and REPL-driven testing; production work may also use REPL-like inspection and
interactive operations. The shell must therefore avoid a hard split where Dev is
only local and Ops is only production.

The current runtime top-level workspaces are:

- Home;
- Dev;
- Ops.

These workspaces remain the production implementation baseline while the
generic visual and interaction foundation is designed. They are not a
constraint on the later goal-led flow review: workspace grouping, navigation,
and workflows may be retained, renamed, regrouped, or replaced after the
foundation is accepted. Foundation work must not change runtime behavior merely
to anticipate that later review.

## Generic Foundation Review

The preview starts with focused, workflow-neutral cases for shell chrome,
content density and states, splits and focus, leader shortcuts, search overlays,
and decision overlays. Workflow-specific cases are added only when their flow
is actively being reviewed and retained afterward when they provide regression
value.

Shortcut visibility uses progressive disclosure:

- the bottom control zone starts with workspace navigation;
- one persistent row shows global actions;
- workspace and focused-pane actions merge into one contextual row when they
  fit, and use a second contextual row only when required by available width;
- `ctrl+x` appends a transient row of structured leader choices;
- inputs and modals show their local navigation and accept/cancel controls;
- help is the complete reference.

Contextual quick-hint rows keep their structural spine at the left edge and
align the visible shortcut group to the right edge. Modal-local shortcut rows
remain centered, and workspace navigation retains its joined left-to-right
layout.

Pane titles contain identity only, never shortcut hints. Active pane titles use
square delimiters and inactive pane titles use round delimiters so focus remains
visible without color. Titles join directly to their owning boundary without
gaps, adjacent rails, or stacked horizontal rules.

Foundation decisions are reviewed in dependency order: visual tokens, shell
geometry, content density, panels and splits, shortcut grammar, overlays, then
compact behavior. The compact case is added after the normal-sized foundation
is coherent rather than maintained as a separate visual language.

The TUI must not add top-level workspaces for Jobs, Observe, Context, Settings,
Server, Output, or REPL. Those are global surfaces, panels, sub-pages, or jobs
inside the three workspaces.

Terms:

- Workspace: a top-level area. The initial workspaces are Home, Dev, and Ops.
- Sub-page or tab: a stable page inside a workspace.
- Panel: a named layout region inside a preset layout.
- Focus: the active panel or control that receives keyboard input.

Every workspace body uses the pane model. A layout with one pane is not a
separate single-view mode; it is a one-pane layout with one focused pane. Focus
switch actions appear only when another pane is reachable.
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

The persistent footer names concrete scoped actions rather than a generic
`ctrl+x More` entry. At narrow widths it keeps whole actions in priority order
and exposes `ctrl+p Commands` when lower-priority actions do not fit.

Workspace navigation is the first row of the bottom control zone and joins its
top boundary using the same connected geometry as pane titles. Global,
workspace, and focused-pane rows continue beneath it on one left spine, which
opens with `┌` on the identity row and closes with `└` on the final row.

The workspace selector containing `1`, `2`, and `3` is always visible. The
remaining global, workspace, and focused-pane shortcut-hint rows will be
toggleable as a group, without hiding or moving that primary selector.

Structural decorators never change color to communicate focus or mode. Rails,
junctions, rules, and popup borders use one neutral decorator color; semantic
status glyphs and active pane titles may still use semantic or accent colors.
The top terminal-facing pane-title rule ends with `┐`, additional stacked title
rules end with `┤`, and the `[1, 2, 3]` workspace-selector row ends with `┘`.
These are isolated terminators: no right rail is drawn between them, so panes
retain the terminal-edge content column. Internal split rails join the
workspace-selector boundary with `┴` only when the junction cell contains no
selector content; labels and shortcuts take priority over a decorative
connection.

Scrollbars use neutral gray chrome: muted gray for the thumb and end markers,
and the fainter structural gray for the track. They never use the active orange
accent, which remains reserved for focus and semantic emphasis.

Every vertical scrollbar occupies the trailing-right cell of its pane. A
visible scrollbar structurally reserves exactly one column from that pane's
content rectangle; it never overlays text, the outer-left spine, or a resize
divider. The pane region that owns the content rectangle also owns its
scrollbar slot, boundary rectangles, role, and hit metadata, including for
stacked panes.

Popups and modals use a complete four-sided border with the same narrow
box-drawing weight as pane decorators. Border cells and the inset interior share
the distinct popup surface. Because terminal box glyphs are centered within
cells, a half-cell of popup color remains visible outside the perceived stroke;
this is an accepted terminal-grid compromise in exchange for narrow, connected,
even borders. Border foreground remains the neutral structural color regardless
of popup status.

Overlays may place identity in the top border using `─[ Title ]──`. Brackets are
neutral gray and every title uses the same yellow accent as GOLEM, independent
of status. Warning and error meaning stays in popup content rather than changing
structural title styling. Search uses a dedicated full-width input row with a
lighter input surface and a muted gray `›` prompt. The field is centered within
the popup by the shared content padding and spans the full padded content width.

Search inputs use a muted `›` prompt and a darker green query color derived from
the active/success family. Search results use equal-width centered rows with a
two-cell marker slot, keeping every label on the same left edge. Selection uses
`◆` without shifting the label and fills the entire fixed row with a slightly
darkened light-gray background and dark text. Popup content is centered except
for text typed inside the full-width input field. Popup shortcut rows reuse the
main footer grammar:
lowercase key notation in subdued amber, capitalized muted labels, and centered
row alignment.

Every popup has one cell of content padding inside all four border edges.
Components do not add their own outer margin within that area. Distinct content
groups use one blank row between them; search separates its input, result list,
and shortcut row this way. Confirmation and nested modals also place one blank
row immediately after their leading `!` or `×` message.

Lists of affected jobs in confirmation modals use an equal-width, fixed-column
table with muted `Job` and `State` headers. Fields are left-aligned to stable
column starts while the table as a whole remains centered in the popup.

An overlay never replaces the underlying pane identity. Single-pane overlay
screens retain the selected pane-title row directly beneath the GOLEM context
row, including the connected `┌` to `├` left spine and square active title.

Content hierarchy is semantic and shared across panes. Section headings use a
bold primary title and optional muted detail. Label/value fields align labels to
a caller-declared shared width and keep values primary. Status always combines a
stable glyph and text label so color is supplementary: `●` running, `○` idle,
`×` failed, and `!` attention.

Informational, active, success, loading, warning, error, unavailable, and empty
states use the shared notice representation with a stable glyph, kind label,
and message. The glyph vocabulary is `i` notice, `●` active, `✓` success, `…`
loading, `!` warning, `×` error, `—` unavailable, and `○` empty. Content tables own
their marker slot, fixed column starts, character-safe ellipsis, and full-row
selection surface. Log and output rows reserve a muted stream prefix and
truncate safely to the available row width. These rules are identical in
single-pane, split-pane, scrolling, and long-content layouts; stories may vary
data and geometry but may not recreate the styling locally.

`OutputLine` receives the actual usable row width, including its stream prefix.
When truncation is necessary, the ellipsis occupies the final usable cell; a
caller must not invent trailing padding by subtracting from that width.

Compact fixed-column rows remain suitable for popups and small summaries. Main
pane datasets use the separate pane-table model: columns have stable IDs,
explicit widths, required or optional visibility, and an `ellipsis` or
`wrap-selected` content policy. Unselected records stay one line. Multi-line
selected records are available only with odd/even row surfaces, because the
row background is what keeps a wrapped record visually coherent; its selected
surface covers every line.
The frozen two-cell marker does not pan with the virtual data columns. Selected
records use a solid `▌` rail in the first marker cell, repeated on every visual
line so wrapped records retain one continuous full-height selection edge.

When enabled columns exceed the pane width, plain left and right arrows pan the
table's data viewport while it owns focus. Required columns cannot be disabled.
Optional columns are changed transactionally in a scoped Columns overlay and
remain session-local. Enter opens an optional 60/40 right details pane without
moving focus from the table; selection changes update details, Tab may focus
them, and Escape closes them or returns table focus. Table and details panes
own independent scrollbars and preserve the shared pane boundary rules.

The pane-table decoration is under active review. The candidates are minimal
rows, faint one-cell `│` rules directly between cells without space padding,
and odd/even background rows. Odd/even tables own four surfaces: odd and even,
each in selected and unselected forms. Pane-table selection uses a restrained
fill plus full-height `▌` rail; the brighter selection surface remains reserved
for search overlays.

The Columns overlay uses the compact popup-table grammar with `Column` and
`Visibility` columns, one selected row, required/shown/hidden states, and an
explicit `↑/↓ Navigate` hint alongside toggle, apply, and cancel actions.

Active and success use the positive green semantic family. Orange remains the
focus/accent family and is not used to color active operational state.

The main context header and footer use the normal shell surface rather than a
special filled background. GOLEM is padded bold orange text without a filled
block.

Footer workspace selection relies on square-active/round-idle shape and neutral
text hierarchy rather than the orange accent. Shortcut keys use subdued amber,
darker than the active-pane accent, so they remain recognizable as keys without
competing with pane focus. This includes the `1`, `2`, and `3` workspace keys
inside the footer navigation labels.

Application, environment, and server metadata use uniform `label value` pairs
with a single space between the muted label and bright value, and neutral middle
dots between pairs. A matching dot separates the padded GOLEM
identity from the first application pair. The identity row does not use chips,
path notation, or unconnected vertical dividers for these fields.

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

The earlier Home, Dev, and Ops shell remap remains implemented history. Its
workspace and flow choices are reopened for the goal-led review that follows the
generic foundation.

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
Major workspace or navigation reshaping must wait for the generic foundation
and the subsequent goal inventory.

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

Milestone 7: establish the generic design lab and compact minimal foundation.
Compare Production with Frame Base through focused cases, promote accepted
themed batches, then add compact validation.

Milestone 8: inventory user goals and redefine product flows. Derive navigation,
workspace grouping, and scenario stories from that inventory instead of treating
the current Home, Dev, and Ops model as fixed.

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
