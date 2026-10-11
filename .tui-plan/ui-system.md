# TUI Design System

This is the authoritative design-system document for the Golem TUI. It records
accepted foundation rules and the current interaction model while the broader
product flows are reviewed.

The document is intentionally strict. Future TUI changes must either follow
these rules or update this document first.

## Real-renderer previews

The optional `tui-preview` feature owns browser, terminal, and static-export
tooling without entering the normal CLI build. Run `cargo make tui-preview`
for the watched browser, `cargo make tui-preview-terminal` for the watched
terminal, or `cargo make tui-preview-export` for an artifact under `target/`.

The baseline registry contains production-rendered ready-state fixtures for
Overview, Agents, one full Agent, Oplog, Stream, Metrics Dashboard, Metrics
Explore, Deployments, and Configuration. They are constructed in a
feature-gated child module of the production application and render through
the same Ratatui tree as the live TUI rather than a synthetic parallel
renderer. The browser URL preserves case, font, and font size. Browser
ligatures are disabled; pinned Fira Code and Iosevka Term assets must pass
regular/bold cell-width and joined-vertical-glyph checks before a frame is
shown.

`Production` is the only visual style. There is no dormant Frame Base,
candidate, comparison, or coverage machinery. Add a focused synthetic lab
module only when a concrete design session needs one, keep it behind
`tui-preview`, and remove it after acceptance unless it provides unique
regression value. Track review outcomes in `dx-ux-review.md`; durable rules
belong in this document and their executable production representation.

### Programmatic design sources

A locked visual decision must have one executable representation in addition
to its written rule. Use a named widget, semantic token, typed rendering model,
or tested layout invariant according to the kind of decision. Preview stories
provide content and application state, but must not recreate accepted glyphs,
spacing, alignment, colors, selection treatment, or boundary behavior locally.

The shared component system owns context headers, pane boundaries, workspace
selectors, shortcut rows, scrollbars, popup frames and titles, search inputs,
selectable rows, decision-table rows, section headings, label/value fields,
status markers, notices, and pane tables. `TuiVisualStyle` owns
their semantic colors. Explicit pane and footer geometry owns junction
placement. The accepted foundation is promoted into Production; production
screens and preview stories must compose the same components rather than
recreate them locally.

`PaneLayout` is the executable source for weighted horizontal content geometry;
`PaneChrome` applies the same accepted header, body-spine, divider, junction,
content-inset, and scrollbar ownership rules to recursive right/bottom layouts.
Pane content must not add a rail beside an internal divider. The
terminal-facing right edge remains title-only `┐` plus the workspace-row `┘`,
with no body rail. Body boundaries render after pane content so content cannot
punch blank cells through the outer left spine or shared dividers. Each visible
right-side divider has a short orange heavy-line grip centered on the otherwise
neutral rail. A bottom divider reuses the downstream pane's title row instead
of adding another horizontal rule or grip. The entire vertical rail or reused
title row remains the resize hit target.

When a decision is revisited, change its existing representation and focused
tests. Temporary lab alternatives are removed when the choice is locked.

Preview fixtures are deterministic and offline; they do not exercise providers
or process lifecycles. Use `cargo make tui-preview-live -- <global flags>` to
launch the checkout-built production `golem <global flags> tui` runtime. The
terminal preview catches watcher termination and returns through its raw-mode
and alternate-screen cleanup guard before it is rebuilt. Neither preview
watcher supervises the live runtime.

## Core Model

The TUI is a keyboard-first shell over one explicit app/env/server context. Dev
and Ops are the first top-level workspaces, but workspace navigation must scale
to later additions. Dev owns application development workflows; Ops owns
deployed-resource exploration, signals, and contextual operations. Switching
between them preserves context, processes, focus, navigation, and layout.

The complete behavioral contract and vocabulary live in product-contract.md.
This document owns their visual and interaction representation.

## Preview Review Policy

The persistent preview baseline is the production-rendered Ops destination
catalogue. Add loading, empty, partial, unavailable, stale, or failed cases
only when those states materially differ from their ready-state destination or
when a concrete design question requires them. Retain a case afterward only
when normal production render tests cannot provide the same regression
coverage.

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

The TUI must not add top-level workspaces for Home, Jobs, Observe, Context,
Settings, Server, Output, or REPL. Those are global surfaces, Dev views,
subject views, panes, or sessions.

Terms:

- Workspace: a top-level area. The initial workspaces are Dev and Ops.
- Dev view: Build & Deploy, Server, or REPL.
- Primary view: the Dev view occupying the main layout region.
- Subview: an optional secondary Dev view beside the primary view.
- Ops destination: a stable top-level page in the Ops tab row.
- Ops route: a root or drill-down state owned by one destination.
- Pane: a focusable rendered layout region. Details is a pane, not a subview.
- Focus: the active pane or control that receives keyboard input.

Every workspace body uses the pane model. A layout with one pane is not a
separate single-view mode; it is a one-pane layout with one focused pane. Focus
switch actions appear only when another pane is reachable.
- Job: a finite command or resource operation with status, output, and launch
  context. Server, REPL, and Live are persistent sessions instead.
- Context: the global selected target used by future actions. A context is not
  just an environment name; it is a resolved app/environment scope plus a
  server target, with enough selector provenance to explain why it is
  available.

## Global Shell

The header must show the selected context clearly. Context includes enough
target information for the user to understand where future actions will run.
The header is context-only: it uses the full `app value · env value · server
value` grammar or the compact `app/environment/server` path. Workspace identity
belongs in the footer selector, view identity belongs in pane titles, and Job or
persistent-session activity belongs in its owning pane or overlay. The header
must not add Dev/Ops, view, Job, Server-session, or REPL-session badges.

Context switching must be a global picker or modal opened from the header,
palette, or shortcut. Context switching must not be a top-level workspace.

Settings must be reachable through palette, modal, or scoped actions. Settings
must not be a top-level workspace unless this document is revised.

Commands is one universal jump surface with All and Context scopes. `ctrl+p`
opens the scope last used in the current TUI session (All in a fresh session),
`ctrl+x a` always opens Context, and left/right or mouse switches the scope
without clearing the current query. It should be able to search
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

Command and session review forms use a centered `[ Run ]` primary action with
the orange active background, followed by the standard left-aligned selectable
field table. Per-launch confirmation options stay at the top: `--yes` first for
commands and REPL, and clean then `--yes` for Server. Free-text fields enter a
transactional editor with the shared input background and a visible terminal
cursor; Enter applies and esc discards the draft.

## Workspaces

Dev contains Build & Deploy, Server, and REPL. One is primary and the others may
be shown as right or bottom subviews. There are no named presets. The first Dev
visit shows Build & Deploy alone when available. Hidden Server and REPL sessions
continue running, and returning to Dev restores the session layout.

Direct navigation to a Dev view shows and focuses it without hiding any other
view or changing which view is primary. Layout actions independently toggle
Build & Deploy, Server, and REPL visibility, refuse to hide the final visible
view, and can promote the focused view to primary without changing the visible
set. The visible set and primary view are session state; placement and split
ratios are visual preferences.

Build & Deploy owns Plan, Build, Deploy, Clean, and their finite Jobs. Server
owns local lifecycle, process state, bound ports, data directory, and logs. REPL owns its
interactive PTY. View capability is independent: unavailable views stay
discoverable and explain what context is missing.

Ops has one persistent horizontal tab row: Overview, Agents, Oplog, Stream,
Metrics, Deployments, and Configuration. A destination may own breadcrumbs and
drill-down routes, but selecting an entity never silently changes another
destination's scope.

Overview is a single-pane cockpit, not a card dashboard. It uses compact status
rows followed by shared `NavigationRow` entries with stable label and
description columns. Short layouts may omit the status block and keep the
selected quick jump visible; they do not collapse the directory into a new
navigation language.

Every Ops destination uses the same page hierarchy. Its first content row is
an identity-only page or route title. View, source, order, liveness, and scope
choices belong to a shared segmented `ControlStrip` beneath it; loading,
refresh, and continuation state belongs to `PaneHeaderStatus`, not to the
title. Nested data panes use `PaneChrome`, continuous outer-left spines, and
owned structural dividers. Responsive stacking changes geometry only: it does
not add duplicate headings or a destination-specific control grammar.

Metrics separates curated Dashboard from ad-hoc Explore and keeps Server and
Agent telemetry explicit. Oplog separates Grouped from exact Entries. Stream
is an exact-Agent live session. Fake telemetry belongs only to visibly labelled
preview/test providers and must never appear as live production data.

Shared resources have one entity identity. Dev may link to an Ops destination,
but must not invent a duplicate Dev-specific resource explorer.

## Layout And Navigation

Workspace navigation lives under the ctrl+x w group. It provides next,
previous, list, and direct mnemonic destinations and must scale beyond Dev and
Ops. It avoids alt+number, ctrl+arrow, and punctuation-heavy bindings.

View navigation lives under ctrl+x v. Workspace selectors and view selectors
represent stable product destinations, not user-opened documents. In Dev, the
three direct destinations always mean show and focus Build & Deploy, Server, or
REPL. They do not start a process and do not silently rotate the layout.

Dev split geometry is derived from primary view, visible subviews, and right or
bottom placement. Ops uses the same split machinery for collection/details.
Every split has explicit pane identity, focus, ratio, minimum size, divider,
and hit metadata.

Focus movement must be keyboard-first. The UI should provide:

- `tab` as the simple focus cycle inside the current layout;
- direct focus shortcuts for important panels;
- contextual help that names the focused panel and reachable focus targets.

Pointer interaction should become native to the layout model, while remaining
optional for primary workflows. Scrolling must affect the panel under the
pointer, clicks should focus tabs, splits, buttons, and selectable rows, and
split handles are resizable by pointer.

ctrl+x l enters temporary Layout mode. Tab and shift+tab choose a pane;
arrows move the applicable divider one cell and shift+arrow moves five cells.
In Dev, `b`, `s`, and `r` toggle the three views, `m` promotes the focused view
to primary, and `f` flips right/bottom placement. In Ops, `d` toggles Details
and `f` flips placement. Enter or esc exits. Compact rendering may temporarily
hide panes but retains the wider-layout ratios and visibility preferences.

Breadcrumbs should be used inside Ops drilldowns and other nested resource
views. Breadcrumbs are for orientation and jumping back up the current resource
path; they are not a replacement for top-level workspace navigation.

Breadcrumb segments should be keyboard-addressable through leader commands,
search, or another explicit keyboard path. They must not be visual-only
navigation.

`esc` steps out. It closes transient modes and modals first, leaves explicit
focus modes next, then exits the current drilldown one level. Normal shell
`esc` should not be the primary quit command.

ctrl+q quits from a normal shell state. If Jobs or TUI-owned sessions are
active, it opens Return/Force quit choices with Return selected. Force quit is
explicit and stops all TUI-owned child processes during cleanup.

The initial navigation history model is back/step-out only. Forward history is
deferred until there is a concrete workflow that needs it.

## Interaction And Shortcuts

Shortcut notation must be lowercase. Documentation should use forms such as
`ctrl+x`, `ctrl+x n`, `ctrl+p`, `esc`, `enter`, and `tab`. It must not use
capital letters in a way that implies `shift` unless `shift` is explicitly part
of the shortcut.

Command shortcuts must not consume bare printable keys. Commands use `ctrl`,
`alt`, or the `ctrl+x` leader so ordinary typing can belong to the focused pane.
Direct non-printable structural controls such as arrows, `enter`, `esc`,
`pageup`, `pagedown`, `home`, and `end` remain unmodified. In a pane with a
primary Find or filter field, typing should enter that mode and insert the
character without requiring a separate activation shortcut.
Entering a text-editing mode transfers only bare printable input. Modified
global and pane shortcuts remain active while the field is being edited, so a
user can open Commands or Help, change pane focus, resize a split, refresh, or
apply loaded-match actions without first leaving Find.
Every active text field uses the shared input surface and owns a visible native
terminal cursor. Long values keep the insertion end visible without splitting
Unicode characters. Enter applies a transactional draft and esc restores the
last applied value unless the field is explicitly documented as live input.

Tab and shift+tab are reserved for pane focus. They do not switch workspaces,
views, Activity representations, palette scopes, or Find fields. ctrl+arrow is
avoided because macOS uses it for Spaces navigation, and alt+number is avoided
because it produces characters on some keyboard layouts. The Columns overlay
deliberately permits bare space to toggle its focused row.

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

Leader commands use stable mnemonic groups:

- ctrl+x w: workspace next, previous, list, and direct destinations;
- ctrl+x v: views or representations within the current product scope;
- ctrl+x l: temporary Layout mode;
- ctrl+x a: contextual actions;
- ctrl+x j: Jobs;
- ctrl+x e: context.

Only common leaf actions deserve dedicated sequences. The global palette
remains the complete searchable surface.

Leader hints should show immediate available keys. Scoped help should provide
the full reference for the current workspace, sub-page, panel, and focus.
Help and Commands use left-aligned, fixed-height table rows with stable key,
label, shortcut, and description columns. Long content ellipsizes within its
column; it must not wrap one action across multiple visual records.

The persistent footer names concrete scoped actions rather than a generic More
leader entry. It also carries one `─( Server … · REPL … )─` session-status
cluster in both workspaces. A status item uses the full state at normal widths
and a semantic glyph at compact widths. Clicking it shows and focuses the
owning Dev view; clicking the same visible, focused item again hides the pane
when another Dev view remains. Neither action promotes, starts, stops, or
transfers terminal input. At narrow widths the footer keeps whole actions in priority order
and exposes `ctrl+p Commands` when lower-priority actions do not fit. A fallback
belongs only to the row that owns that action; contextual overflow uses a
neutral ellipsis and must not duplicate a global shortcut in another row.

Workspace navigation is the first row of the bottom control zone and joins its
top boundary using the same connected geometry as pane titles. Global,
workspace, and focused-pane rows continue beneath it on one left spine, which
opens with `┌` on the identity row and closes with `└` on the final row.

The Dev/Ops workspace selector is always visible when size permits. Additional
workspace destinations use the same ordered selector model. Remaining global,
workspace, and focused-pane shortcut hints may collapse without hiding or
moving the primary selector.

Production enters its compact shell below 72 columns or 20 rows. Compact mode
keeps the workspace selector and one global Commands/Help/Quit row, while
omitting contextual hint rows so the body retains vertical space. The selector
keeps the canonical short workspace name without clipping.
`SHELL_COMPACT_WIDTH`, `SHELL_COMPACT_HEIGHT`, `WorkspaceSelector`, and focused
production render tests are the executable contract for these reductions.

Structural decorators never change color to communicate focus or mode. Rails,
junctions, rules, and popup borders use one neutral decorator color. The short
resize grip is an accent affordance on a standalone vertical divider; a stacked
pane's title rule is itself the horizontal resize affordance. Semantic
status glyphs and active pane titles may still use semantic or accent colors.
The top terminal-facing pane-title rule ends with `┐`, additional stacked title
rules end with `┤`, and the workspace-selector row ends with `┘`.
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
stacked panes. Clicking the track or dragging the thumb scrolls only that pane;
scrollbar hit regions take precedence over and remain disjoint from split
resize handling.

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
`▌` without shifting the label and fills the entire fixed row with a slightly
darkened light-gray background and dark text. Semantic notices and primary
actions are centered in popups; form, help, command, and decision-table rows
retain their shared left-aligned columns. Popup shortcut rows reuse the main
footer grammar:
lowercase key notation in subdued amber, capitalized muted labels, and centered
row alignment.

Every popup has one cell of content padding inside all four border edges.
Components do not add their own outer margin within that area. Distinct content
groups use one blank row between them; search separates its input, result list,
and shortcut row this way. Confirmation and nested modals also place one blank
row immediately after their leading `!` or `×` message.
Every reachable popup, including Commands, Help, context selection,
confirmation, and blocking loading state, uses the shared overlay frame,
notices, selection rows, and shortcut row rather than a local border or rail
variant.

Data-browsing overlays—Help, Commands, context selection, Dataset, and
Columns—adapt toward 85 percent of both terminal dimensions while retaining a
one-cell outer margin where possible. Their control footer stays pinned while
the body scrolls, and an overflowing body reserves one trailing-right cell for
its scrollbar. Compact confirmations and blocking/loading dialogs remain sized
to their action and message content instead of expanding to the data-overlay
footprint.

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

Within Ops pages, section headings are content labels rather than substitutes
for pane identity. A page must not repeat its destination title in a second
heading, embed mode or scope metadata in that title, or join unrelated state
with decorative dot separators. Use the page control strip and pane-header
status slot for those roles.

Informational, active, success, loading, warning, error, unavailable, and empty
states use the shared notice representation with a stable glyph, kind label,
and message. The glyph vocabulary is `i` notice, `●` active, `✓` success, `…`
loading, `!` warning, `×` error, `—` unavailable, and `○` empty. The glyph and
label form a bracketed badge—such as `[i Notice] message` or `[○ Empty]
message`—so the marker, state name, and message have explicit visual
boundaries. The glyph keeps its semantic color while the label and brackets use
the shared muted structural hierarchy. Popup notices wrap as centered styled
lines, while inline notices remain left-aligned. Content tables own
their marker slot, fixed column starts, character-safe ellipsis, and full-row
selection surface. Log and output rows reserve a muted stream prefix and
truncate safely to the available row width. These rules are identical in
single-pane, split-pane, scrolling, and long-content layouts; stories may vary
data and geometry but may not recreate the styling locally.

CLI and server diagnostics entering semantic notices are stripped of ANSI
control sequences. The TUI applies its own styles; embedded terminal escapes
must never appear as visible error-message characters.

`OutputLine` receives the actual usable row width, including its stream prefix.
When truncation is necessary, the ellipsis occupies the final usable cell; a
caller must not invent trailing padding by subtracting from that width.

Compact fixed-column rows remain suitable for popups and small summaries. Main
pane datasets use the separate pane-table model: columns have stable IDs,
explicit widths and required or optional visibility. The Agents collection is
single-line because Details owns full values; columns resolve from their
minimums toward configured and observed content widths before horizontal
panning is needed.
The frozen two-cell marker does not pan with the virtual data columns. Selected
records use a solid `▌` rail in the first marker cell.
AgentID cells reuse the CLI formatter's semantic tokenization: strings use the
positive color, numbers the informational color, literals the warning color,
and punctuation the muted color. The same token colors apply to the inline or
structured multiline AgentID in Details. Ellipsis and row selection preserve
those foreground distinctions over the selected row background.
Numeric data columns are right-aligned. Responsive sizing protects AgentID's
minimum and useful width ahead of Type and Component when space is tight.

When enabled columns exceed the pane width, plain left and right arrows pan the
table's actual rendered data viewport while it owns focus. Required columns
cannot be disabled.
Optional columns are changed transactionally in a scoped Columns overlay and
persist as visual preferences. Details visibility and right/bottom placement
belong to Layout actions; Enter opens the selected resource subject. Selection
changes update visible Details without stealing focus. Pointer clicks or tab
and shift+tab move focus between collection and Details, and Layout mode or
pointer dragging resizes their shared divider. The
details document scrolls independently with arrows, paging keys, boundaries,
and the mouse wheel. Its AgentID remains inline when it fits and uses the CLI's
structured multiline formatter otherwise. Status is semantically colored, and
agent-type plus instance metadata use the shared colored-JSON document.
The agent list itself accepts the mouse wheel under its pointer region and moves
the focused selection through loaded logical rows. Loading another batch is
always explicit through `ctrl+l` or Commands; navigation never triggers it.
Split focus and resize shortcuts are present in contextual footer hints whenever
the details pane is visible and in scoped Help.
Table and details panes own independent trailing-right scrollbars and preserve
the shared pane boundary rules.

Main-pane tables use one-line odd/even background rows. Their four surfaces
cover odd and even rows in selected and unselected states. Pane-table selection
uses a restrained, desaturated amber fill plus an accent-colored full-height
`▌` rail; the brighter selection surface remains reserved for search overlays.
Faint one-cell `│` rules directly between cells without space padding remain
available where separators improve scanning. Full or wrapped values belong in
Details rather than collection rows.

Rows below a short table return to the pane surface across the complete pane
width; virtual table width must not leave a differently shaded text-width
rectangle behind.

The Columns and Dataset overlays use the compact popup-table grammar with a
one-cell `│` separator and no padding around that separator. Columns has
`Column` and `Visibility` fields, one selected row, required/shown/hidden
states, and an explicit `↑/↓ Navigate` hint alongside bare `space` toggle,
apply, and cancel actions. Dataset uses the same grammar for Component and
Agent type so adjacent values can never visually concatenate. It also accepts
case-insensitive literal type-ahead over those two fields, keeps the match count
visible, and uses `ctrl+u` to clear the query. Popup selection rails always use
the accent foreground independently of the selected-row background.

Agents Overview offers AgentID, Status, Type, Component, Revision, Pending, and
Created at. AgentID and Status are required; Type and Component are shown by
default; Revision, Pending, and Created at are optional and initially hidden.
AgentID ellipsizes in the table and uses the structured full-value formatter in
Details. Pointer hit testing uses the persisted visible window, so selecting a
visible row does not move it to a different screen position.

Paged collections show two different query layers in a shared query/status
bar. `Dataset` names the exact server-side component/type restriction. `Find`
names a local, case-insensitive literal substring query over one explicit field:
AgentID by default, Component, or Agent type. There is no ambiguous `All fields`
scope, fuzzy subsequence matching, or relevance reordering; matching rows retain
their loaded order. The bar reports
the real loaded count and `more available` whenever any server cursor remains;
it never displays a guessed total or synthetic page number. Explicit All/None
actions affect only loaded Find matches. Each request loads at most 200 rows
total across every matching component, and only the explicit Load Next 200
action advances the typed page cursor. Refresh and auto-refresh reload the
already requested depth without discovering the next batch. A pane-header
status separately shows refreshing, loading-next, auto-refresh-active, or
auto-refresh-off state. Active and loading statuses use square brackets; idle
statuses use parentheses. Refresh and loading-next states animate their spinner.
The current Durable/Ephemeral/All Agent mode is rendered in the same bar and is
never left implicit.

Active and success use the positive green semantic family. Orange remains the
focus/accent family and is not used to color active operational state.

The main context header and footer use the normal shell surface rather than a
special filled background. GOLEM is padded bold orange text without a filled
block.

Footer workspace selection relies on square-active/round-idle shape and neutral
text hierarchy rather than the orange accent. Shortcut keys use subdued amber,
darker than the active-pane accent, so they remain recognizable as keys without
competing with pane focus.

Application, environment, and server metadata use uniform `label value` pairs
with a single space between the muted label and bright value, and neutral middle
dots between pairs. A matching dot separates the padded GOLEM
identity from the first application pair. In compact mode, the same values use
the existing `app/environment/server` path grammar without labels. The identity
row does not use chips, new punctuation conventions, or unconnected vertical
dividers for these fields. `ContextHeader` selects the full or compact grammar
from the available width before text clips.

Text input owns printable keys. When a filter, search box, command input, or
other text field is focused, printable keys must type into that field. Only
escape, control keys, leader commands, and explicit accept/cancel keys should
interrupt text entry.

All nested terminals use the same explicit ownership model and display the
current owner. Launching a command or session switches to Dev and reveals and
focuses its owning view without promoting it or hiding another view. A finite
command or REPL receives CLI input in that focused pane. Server always starts
in TUI control so an unexpected server prompt cannot capture shell navigation.
ctrl+x returns to
persistent TUI control; Enter or clicking a running focused pane gives input to
the CLI. While the CLI owns input, every other key, including tab and ctrl+c,
reaches the PTY. The terminal summary reports `input CLI` or `input TUI` but
does not repeat the ctrl+x handoff action shown in the footer.

Lists and tables should use arrow-first navigation:

- `up` and `down` move selection;
- `pageup` and `pagedown` move by page;
- `home` and `end` jump to boundaries;
- `enter` opens or drills into details.

Enter on a row opens its resource subject. Details remains a live preview. The
contextual palette exposes actions for the focused subject or explicit
selection. Destructive or mutating row actions never rely on bare direct keys.

Shortcut resolution is scope-first:

- focused input or panel;
- workspace or sub-page;
- global shell.

Help must show the active meaning of a reused shortcut. Avoid reuse where it
would be surprising, but reuse is allowed when focus makes the meaning clear.

## Jobs And Output

Output is not a top-level workspace. Finite commands and resource mutations are
Jobs with kind, status, risk, immutable target, masked inputs, structured result
or output, timing, exit state, and error. Jobs appears as a global overlay and
Build & Deploy embeds only its relevant subset.

The shell always exposes running or failed Jobs. Navigation does not cancel a
Job. Only one mutating Job may run per application environment; other mutations
are disabled with the active Job named and are never queued automatically.

Server, REPL, and Live are persistent sessions rather than Jobs. Each has its
own status, buffer or terminal state, and stop policy. Hiding its pane or
switching workspaces does not stop it.

Server and REPL lifecycle commands are state-specific and never double as
navigation. Stopped and failed sessions expose Start. Starting and running
sessions expose Stop and Restart. Stopping sessions expose Force Stop only
after a five-second grace period. Starting, stopping, or restarting a session
does not rearrange the Dev workspace. The footer status cluster is the global
visibility affordance for these two sessions; Live remains scoped to Activity.

Context switching confirms and stops context-bound Jobs and REPL. Server may
continue only when the server target is unchanged. Live closes on context
change. Active-work quit uses the explicit Return/Force quit dialog.

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
defined by that manifest. Capabilities are evaluated per view rather than by a
single Dev-enabled flag: Build & Deploy needs a manifest project, Server needs
a local target, and REPL needs an app environment plus reachable server.

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
known to that server. That is a valid Dev context even without a matching local
manifest: REPL may be available while Build & Deploy is not. Ops may run against
any target with enough information for the requested subject or action.

Local Server is a persistent Dev session bound to an exact server target, not a
global drawer and not a finite Job. Leaving Dev does not stop it. Switching
between environments on the same server may preserve it; changing server target
requires stopping it or cancelling the switch. Selecting an unreachable local
target reports unavailability and never auto-starts it.

Server process lifecycle and bound-port observation are independent. Successful
spawn is Running even when no ports file was requested. Ports file starts empty
and adds `--ports-file` only when the user supplies a path; a fresh parseable
file enriches the summary without acting as a readiness gate.

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
- hierarchical leader path when useful;
- global/contextual palette relevance;
- category, workspace, subject, and view scope;
- scope;
- availability rule and reason;
- execution kind;
- risk class and review/confirmation policy;
- resolved target summary.

Palette rows, footer hints, leader hints, and help should derive from action
metadata where practical.

ctrl+p opens the palette's session-remembered scope and ctrl+x a opens Context.
Left/right or the scope tabs switch scope, preserve the query, and reset
selection to the best first match. Context includes relevant unavailable
actions and their reasons. A fresh session starts on All.

Non-empty palette queries rank exact labels, label prefixes, word-prefix label
matches, label substrings, fuzzy labels, then fuzzy visible metadata. Score,
availability, and registry order break ties in that order. Empty queries retain
registry order. Only visible row text is searchable, and every matched
character receives the dedicated dark-amber match background.

Non-local mutations are disabled until the exact app/env is unlocked for the
current session. Read-only, routine-write, disruptive, and destructive actions
use progressively stronger review. Destructive confirmation is cancel-focused
and requires a typed target. Unlock never bypasses action-specific safeguards.

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

Production has Dev and Ops workspaces. Dev owns reviewed nested-CLI Plan,
Build, Deploy, Clean, Server, and REPL sessions. Ops has the accepted Agents
collection/details surface plus prototype subject/view pages for deployment,
environment, server, Oplog snapshots, and local Metrics. The prototype data
paths are live, but their navigation and page composition are being replaced by
the stable destination model in `product-contract.md`. Recursive right/bottom
pane layouts, focus, resize, terminal scrollback, context switching, and
bounded finite-Job history are live.

Production composes the accepted `ContextHeader`, `PaneHeader`, status,
boundary, scrollbar, workspace-selector, shortcut-row, notice, table, and
overlay primitives. Dev and Ops share one context-only header, title grammar,
neutral structural chrome, and semantic status colors. The fake OTLP lab was
retired; production Metrics uses only live local-server data.

The active rebuild adds a real Stream session, Dashboard/Explore, bounded Oplog
search, Deployment history/diff, and typed Configuration. Remote metrics,
remote unlock, Ops mutations, and neutral target materializers beyond resolved
CLI flags remain follow-up work. Selected-target launch material is
consolidated and cross-Server switching stops the supervised Server while
same-Server app/environment switches preserve it.

## Review Status

The visual system, live Agents baseline, and product topology in
product-contract.md are accepted. This document is synchronized to that
contract; tasks.md is the authoritative readiness ledger for unchecked design
gates and implementation work.

The Ops rebuild is decision-complete. Checkpoint readiness and implementation
status live in `tasks.md`.

## Roadmap

The executable order is maintained in phases.md and tasks.md:

1. establish stable Ops routes and reusable page primitives;
2. complete local Metrics data and the Dashboard/Explore UI;
3. add bounded Oplog and persistent Stream investigation;
4. add Deployment history/diff and full typed Configuration;
5. finish Overview, preview coverage, and terminal acceptance.

## Deferred Questions

Remote metrics acquisition, Ops mutation safety, persisted metric rollups,
guest-defined metric APIs, Docker/Grafana integration, and a possible web UI
remain deliberately deferred. They are not decisions for the active rebuild.

## Accessibility And Stability

The UI must not rely on color alone for running, selected, disabled, or error
state.

Labels should remain stable and should not cause layout reflow when toggles or
status indicators change.

Nested CLI output and REPL terminal screens should preserve ANSI output. The
Golem TUI theme should style the surrounding chrome, not rewrite interactive
program output.
