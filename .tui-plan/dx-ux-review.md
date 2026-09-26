# TUI DX/UX Review

This document is the operational ledger for iterative TUI design review. It
tracks feedback from deterministic preview stories until each note is accepted,
deferred, or incorporated into an authoritative plan document.

## Review Workflow

1. Start the watched browser gallery with `cargo make tui-preview`.
2. Review the one current focus through Frame Base and at most three mutations.
3. Record small observations as scratch bullets under that focus and mutate the
   existing candidates instead of adding variants.
4. Fold the selected candidate into Frame Base and codify it in a named widget,
   token, typed model, or tested layout invariant.
5. Delete every mutation, validate coverage, synchronize the design-system
   rule with its programmatic representation, and advance the queued focus.

`Production` is the runtime baseline and `Frame Base` is the preview-only design
baseline. The default browser and terminal views show only the current focus;
Production and full coverage are secondary tools. Promotion into Production is
a separate explicit decision and never creates a user-selectable theme.

## Current Focus

- Focus: Ops-first production rebuild (`UX-022`)
- Cases: `ops-agents`, `ops-metrics-demo`, `activity-timeline`,
  `activity-journal`, plus `shell-compact`
- Decision: promote the accepted foundation as the sole production visual
  system, start directly in Ops, and rebuild around Agents Overview and a
  permanently labelled fake OTLP explorer for Metrics layout review.
- Options: no legacy visual variant. Activity is preview-only until its direct
  typed provider is ready.
- Scratch observations: the focused row and explicit cross-agent selection are
  separate states. The frozen marker slot must communicate both without
  changing column alignment. Reachable overlays now share the accepted frame,
  notice, selected-row, and shortcut grammar. Agent Find retains modified
  shortcuts, and pane focus/resize plus list mouse scrolling are discoverable
  and covered by interaction tests.
- Queued next: live Agents Overview visual acceptance
- Exit: production and deterministic previews share the same shell/table
  components, legacy workspaces are unreachable, and wide/compact agent and
  metric states are accepted.

The `shell-compact` case reuses the default shell content at 50×16 so compact
behavior can be evaluated without introducing another workflow or visual
direction.

## Sources Of Truth

- Durable product, visual, and interaction rules belong in `ui-system.md`.
- Approved executable work belongs in `tasks.md`.
- Completed batches and validation history belong in `progress.md`.
- Rendering and scenario validation policy belongs in `testing.md`.
- This file indexes active review state and links to those outcomes instead of
  duplicating their final wording.

Synchronize the relevant source-of-truth document when a note becomes
`approved` or `accepted`. Acceptance requires naming the component, token,
typed model, or tested invariant that enforces the decision. Do not treat
conversation history, scene-local drawing code, or preview-only variants as
authoritative design state.

## Note Format

Use stable IDs of the form `UX-001`. A note is decision-ready when every field
below is concrete enough to approve without another implementation decision.

```markdown
### UX-001 — Short title

- Status: captured | needs-decision | ready | approved | implemented | accepted | deferred
- Preview: <stable gallery link>
- Area: visible workspace, panel, overlay, or shared component
- Theme: layout | navigation | content | interaction | visual-system | responsive | fixture
- Problem: what is confusing, missing, visually weak, or inconsistent
- Desired outcome: observable user-facing result
- Scope: affected stories and shared surfaces
- Dependencies: note IDs, design decisions, or `none`
- Acceptance: what must be visible or behave differently in preview/live use
- Outcome: authoritative document section, task, progress entry, or deferral reason
```

## Status Rules

- `captured`: normalized but not yet checked for ambiguity or dependencies.
- `needs-decision`: requires an explicit product or design choice.
- `ready`: decision-complete and eligible for a themed batch.
- `approved`: included in an explicitly approved implementation batch.
- `implemented`: coded and validated, awaiting visual review.
- `accepted`: reviewed, codified programmatically, tested, and synchronized into
  the appropriate sources of truth.
- `deferred`: intentionally postponed with a recorded reason or dependency.

## Story Coverage

Record the most recent review status and active note IDs for each preview case.
The preview case ID, rather than its underlying fixture name, is the stable
identity; this keeps compact and wide cases independently reviewable.

| Preview case | Last reviewed | Active notes |
|---|---|---|
| `shell-default` | — | `UX-003`, `UX-004`, `UX-005`, `UX-007`, `UX-008` |
| `content-density` | — | `UX-003`, `UX-007` |
| `split-focus` | — | `UX-003`, `UX-004`, `UX-006`, `UX-007`, `UX-009` |
| `shortcut-leader` | — | `UX-003` |
| `overlay-search` | — | `UX-003`, `UX-004`, `UX-006` |
| `overlay-decision` | — | `UX-003`, `UX-004`, `UX-006` |

## Themed Batches

Before implementation, record the batch name, included note IDs, affected
stories, intended outcome, and validation. Mark the batch approved only after
explicit review.

### Remove the Home logo background

- Status: accepted
- Notes: `UX-001`
- Cases: retired workflow cases `home-idle`, `home-active`, and `home-compact`;
  verify in the live runtime when Home is next reviewed
- Validation: focused Home render test and all deterministic preview cases

### Make browser typography terminal-grid safe

- Status: accepted
- Notes: `UX-002`
- Stories: all browser preview cases
- Validation: layered exporter tests, font-policy tests, all deterministic
  preview cases, and manual forwarded-browser review

### Establish the generic design lab

- Status: implemented; awaiting review
- Notes: `UX-003`
- Cases: all six generic design-lab cases
- Validation: every case under Production and Frame Base; preview exporter tests,
  and production wrapper equivalence

### Iterate on Frame Base

- Status: active exploration
- Notes: `UX-004`
- Cases: `shell-default`, `split-focus`, `overlay-search`, `overlay-decision`
- Validation: Frame Base structural assertions and all design-lab cases under
  Production and Frame Base

### Compare pane boundaries

- Status: implemented; awaiting review
- Notes: `UX-009`
- Cases: `split-focus`
- Validation: four-card focus filtering, structural boundary assertions, every
  case under active variants, and production wrapper equivalence

## Active Notes

### UX-001 — Remove the Home logo background

- Status: implemented
- Preview: production live runtime; the original Home preview cases were retired
  when the generic design lab replaced the workflow catalog
- Area: Home workspace background
- Theme: visual-system
- Problem: the large Braille Golem logo depends on platform glyph fallback and
  garbles browser previews, making the surrounding UI harder to evaluate.
- Desired outcome: Home uses the normal surface background without decorative
  logo glyphs.
- Scope: current Production Home rendering.
- Dependencies: future logo placement is a separate deferred decision.
- Acceptance: no Braille logo appears behind Home content at standard, compact,
  or wide terminal sizes.
- Outcome: implementation complete; awaiting visual acceptance.

### UX-002 — Prevent browser font clipping and invalid fallbacks

- Status: accepted
- Preview: all current generic design-lab cases
- Area: browser preview typography
- Theme: visual-system
- Problem: row backgrounds can cover letter descenders at terminal-tight line
  height, while increasing line height introduces gaps in multi-row rails. Font
  choices that silently fall back are not useful for visual review.
- Desired outcome: letters remain visible on a gapless terminal grid, and every
  selectable font is an explicitly loaded, grid-qualified terminal webfont.
- Scope: all browser preview cases; production terminal rendering is unchanged.
- Dependencies: browser access to the pinned font CDN.
- Acceptance: `g`, `p`, `q`, and `y` are not clipped; consecutive `│` and `┃`
  glyphs join without gaps; Fira Code and Iosevka Term visibly differ;
  unavailable fonts block the preview instead of falling back.
- Outcome: accepted in the forwarded macOS browser with both Fira Code and
  Iosevka Term passing grid qualification and visual review.

### UX-003 — Replace workflow previews with a generic design lab

- Status: implemented
- Preview: `http://127.0.0.1:4173/?mode=compare&case=shell-default&variant=Frame+Base&font=fira&size=14`
- Area: preview catalog and shared TUI foundation
- Theme: visual-system | layout | interaction
- Problem: workflow-specific stories force product-flow assumptions into the
  review before the shared visual, layout, focus, overlay, and shortcut language
  is coherent.
- Desired outcome: review a small set of workflow-neutral real-renderer cases
  against one focused direction before redesigning product flows.
- Scope: browser and terminal preview catalogs; production runtime behavior is
  unchanged.
- Dependencies: `UX-002` keeps browser typography reviewable.
- Acceptance: the gallery contains only the six foundation cases; comparison is
  Production and Frame Base; the cases cover shell, content states, mixed
  splits, progressive shortcut disclosure, search, and confirmation.
- Outcome: implementation complete; awaiting visual acceptance and the first
  foundation note batch.

### UX-004 — Clarify Frame chrome and popup boundaries

- Status: needs-decision
- Preview: `http://127.0.0.1:4173/?mode=compare&case=shell-default&variant=Frame+Base&font=fira&size=14`
- Area: Frame-derived shell, overlays, and split decorators
- Theme: visual-system | layout
- Problem: the partially filled header breaks the first-line flow; overlays
  appear as content with an unexplained side rail instead of popups; panel title
  rules, body rails, and split rules stack into visually messy double or triple
  lines.
- Desired outcome: retain Frame's promising title rules while making the header,
  popup boundaries, and split geometry intentional through direct iteration.
- Scope: Frame Base; Production remains the runtime reference.
- Dependencies: `UX-003`.
- Acceptance: the main header is one continuous filled band; overlays have a
  complete border and inset content with no standalone rail; split regions use
  whitespace between panels and never stack title, body, and handle rules.
- Outcome: the temporary derived branches were retired; continue resolving the
  three concerns directly on Frame Base.

### UX-005 — Tighten Frame Base header rail spacing

- Status: implemented
- Preview: `http://127.0.0.1:4173/?mode=compare&case=shell-default&variant=Frame+Base&font=fira&size=14`
- Area: Frame Base shell header and navigation
- Theme: layout | visual-system
- Problem: the header and navigation each retain an extra blank cell after the
  left decorator, making the two top rows feel less compact.
- Desired outcome: test both top rows with their content shifted one cell toward
  the left decorator.
- Scope: Frame Base only; Production retains its existing spacing for comparison.
- Dependencies: `UX-004`.
- Acceptance: Frame Base starts the first row with `│ GOLEM` and the second with
  `│[1]`, without changing body indentation.
- Outcome: implementation complete; awaiting visual review.

### UX-006 — Remove junction glyphs from Frame headers and popups

- Status: implemented
- Preview: `http://127.0.0.1:4173/?mode=compare&case=split-focus&variant=Frame+Base&font=fira&size=14`
- Area: Frame-derived panel headers and unframed overlay rails
- Theme: visual-system
- Problem: `├` reads as a line junction even when no horizontal line joins it,
  making panel headers and popup edges look accidental.
- Desired outcome: use a neutral continuous rail and let the existing marker and
  styling communicate focus.
- Scope: Frame Base.
- Dependencies: `UX-004`.
- Acceptance: no Frame panel header or popup rail renders `├`; structural rails
  use `│` instead.
- Outcome: implementation complete; awaiting visual review.

### UX-007 — Tighten Frame Base panel and view rail spacing

- Status: implemented
- Preview: `http://127.0.0.1:4173/?mode=compare&case=content-density&variant=Frame+Base&font=fira&size=14`
- Area: Frame Base panel titles and view content
- Theme: layout | visual-system
- Problem: panels and views retain a blank cell after their left rail even
  though the main header and navigation no longer use that gutter.
- Desired outcome: align all Frame Base content directly against its rail for a
  consistent compact rhythm.
- Scope: Frame Base only; Production retains its existing spacing for comparison.
- Dependencies: `UX-004`, `UX-005`.
- Acceptance: Frame Base panel markers and view content begin immediately after
  the rail, without changing Production spacing.
- Outcome: implementation complete; awaiting visual review.

### UX-008 — Compare uniform header metadata treatments

- Status: ready
- Preview: queued after the pane-boundary focus
- Area: Frame Base app, environment, and server metadata
- Theme: visual-system | information-hierarchy
- Problem: the identity and context fields do not yet share a deliberate,
  uniform visual grammar.
- Desired outcome: compare label/value pairs, divider-separated pairs, and
  equal-background chips without changing header geometry or behavior.
- Scope: the next preview focus; no header mutations remain compiled meanwhile.
- Dependencies: `UX-004`, `UX-005`.
- Acceptance: once active, Focus mode shows Frame Base and at most three header
  mutations using identical app/env/server data.
- Outcome: queued next so it does not compete with pane review.

### UX-009 — Compare single-owner pane boundaries

- Status: implemented
- Preview: coverage for the completed pane-title focus
- Area: pane title rules, rails, and split handles
- Theme: layout | visual-system | focus
- Problem: the baseline can place panel title rules on neighboring rows and pane
  rails beside split handles, producing `──` and `││` boundaries.
- Desired outcome: retain the selected active-square/idle-round grammar while
  deciding whether pane-local hints improve the title or belong only below.
- Scope: Round Idle is folded into Frame Base; preview mutations now differ only
  in the placement or omission of pane-local shortcut hints.
- Dependencies: `UX-004`, `UX-006`, `UX-007`.
- Acceptance: every candidate preserves the gapless geometry and square/round
  focus distinction; hint placement never changes pane layout or behavior.
- Outcome: Hint None selected. Pane titles use active square and idle round
  delimiters, and shortcut guidance belongs in contextual control surfaces.

### UX-010 — Unify navigation and scoped shortcuts at the bottom

- Status: implemented; awaiting review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=unified-footer&font=fira&size=14`
- Area: shell navigation, scoped shortcuts, and pane model
- Theme: layout | navigation | interaction | responsive
- Problem: workspace navigation is separated from the shortcut footer, while
  pane-local hints compete with pane identity and important controls are split
  between the top and bottom of the terminal.
- Desired outcome: one bottom control zone containing navigation plus persistent
  global, workspace, and focused-pane rows, with a transient leader row.
- Scope: preview-only footer structure and generic always-pane cases; Production
  layout and key handling remain unchanged.
- Dependencies: `UX-009`.
- Acceptance: normal mode has navigation plus one to three scoped rows; unified
  variants contain no generic `More`; compact layouts retain whole prioritized
  actions and a Commands fallback; popup controls remain local.
- Outcome: Unified Joined selected. Awaiting tone and spine treatment.
- Follow-up: make contextual shortcut-hint rows toggleable while keeping the
  `1` / `2` / `3` workspace selector permanently visible.

### UX-011 — Tone down joined shell chrome

- Status: implemented; awaiting review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=joined-chrome-tone&font=fira&size=14`
- Area: footer boundary, footer spine, and context-header background
- Theme: visual-system | layout
- Problem: the selected joined footer is structurally coherent but visually too
  prominent and disconnected in tone from the main context header.
- Desired outcome: a quieter footer rule, one darker shared chrome background,
  and an intentional joined left decorator across footer rows.
- Scope: preview-only Frame Base mutations; Production remains unchanged.
- Dependencies: `UX-010`.
- Acceptance: all candidates retain the selected footer structure, share header
  and footer background colors, use connected left decorators, and differ only
  in depth or whether the bottom spine closes.
- Outcome: Closed Spine selected with the original footer background. Orange
  footer focus and shortcut accents were removed in favor of neutral hierarchy.

### UX-012 — Unify context-header metadata grammar

- Status: implemented; awaiting review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=context-header-metadata&font=fira&size=14`
- Area: app, environment, and server identity metadata
- Theme: visual-system | information-hierarchy
- Problem: context fields need a compact uniform grammar now that header and
  footer share one neutral chrome system.
- Desired outcome: select between labeled values, neutral dividers, a compact
  path, or equal-background chips without adding new focus accents.
- Scope: preview-only header metadata; geometry and Production remain unchanged.
- Dependencies: `UX-011`.
- Acceptance: every candidate preserves neutral decorators, muted footer keys,
  Closed Spine geometry, and the shared base chrome background.
- Outcome: Dividers selected and promoted into Frame Base.

### UX-013 — Remove special shell chrome backgrounds

- Status: implemented; awaiting review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=plain-shell-chrome&font=fira&size=14`
- Area: Golem identity and header/footer surfaces
- Theme: visual-system | information-hierarchy
- Problem: filled shell chrome and the filled GOLEM block compete with the pane
  system even after footer focus colors were muted.
- Desired outcome: evaluate normal surface backgrounds with GOLEM rendered as
  plain orange text, then choose its padding and weight.
- Scope: preview-only header/footer surface and identity styling; Production is
  unchanged.
- Dependencies: `UX-012`.
- Acceptance: plain candidates retain Dividers, Closed Spine, neutral structural
  glyphs, and subdued amber shortcut keys while removing special chrome fill.
- Outcome: Plain Padded selected and promoted into Frame Base.

### UX-014 — Replace unconnected context dividers

- Status: implemented; awaiting review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=context-separators&font=fira&size=14`
- Area: app, environment, and server separators
- Theme: visual-system | information-hierarchy
- Problem: `│` implies a structural line junction even though the header
  dividers do not connect to any surrounding geometry.
- Desired outcome: choose a quieter semantic separator or whitespace-only
  grouping while keeping the selected label/value grammar.
- Scope: preview-only separator glyphs; Production remains unchanged.
- Dependencies: `UX-013`.
- Acceptance: all candidates preserve Plain Padded chrome, neutral structural
  decorators, warm-muted keys, and identical metadata content and spacing.
- Outcome: Dot selected. The same neutral dot separates GOLEM from app metadata.

### UX-015 — Compare context label/value separators

- Status: decided
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=context-value-separators&font=fira&size=14`
- Area: app, environment, and server values
- Theme: visual-system | information-hierarchy
- Problem: plain values may not be sufficiently distinct from their labels in
  the compact dot-separated header.
- Desired outcome: compare whitespace, colon, and equals separators while
  punctuation remains muted and values stay bright.
- Scope: three focused mutations; Production remains unchanged.
- Dependencies: `UX-014`.
- Acceptance: each card renders its declared separator consistently for all
  three values, uses label color for punctuation, and preserves value color.
- Outcome: Space selected. Context pairs use `app value`; colon, equals, and
  angle-bracket mutations were retired.

### UX-016 — Connect pane boundaries to the footer

- Status: decided
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=pane-boundary-connections&font=fira&size=14`
- Area: joined pane-title rules in split layouts
- Theme: visual-system | layout
- Problem: pane-title rules currently stop at the terminal-facing edge without
  an intentional ending.
- Desired outcome: top titles use `┐`, stacked title rules use `┤`, and the
  `[1, 2, 3]` workspace-selector row uses `┘`, without consuming a content
  column between them; split rails join that selector boundary where no label
  occupies it.
- Scope: Frame Base split geometry; Production remains unchanged.
- Dependencies: `UX-015`.
- Acceptance: `┐`, `┤`, and `┘` appear at their respective right-edge rows with
  no rail between them; an unobstructed internal rail meets the selector rule
  with `┴`; selector content is never overwritten; both panel modes are visible
  with edge-reaching content and with actual scrollbar widgets.
- Outcome: accepted. Top titles use `┐`, stacked titles use `┤`, the workspace
  selector uses `┘`, and unobstructed split rails connect with `┴`. Scrollbars
  are gray, always occupy the trailing-right pane cell, and reserve exactly one
  content column only while visible. Non-scrolling content may use the
  rightmost column. Scrollbars, outer-left spines, and resize dividers have
  distinct structural ownership and do not overlap.

### UX-017 — Establish popup and modal hierarchy

- Status: implemented; awaiting baseline review
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=overlay-foundations&font=fira&size=14`
- Area: search popup, confirmation modal, error popup, and nested modal
- Theme: visual-system | overlay | interaction
- Problem: overlays need clear boundaries and depth without reintroducing the
  unexplained side rail or allowing stacked modals to merge visually.
- Desired outcome: define one compact family that distinguishes transient
  search, blocking decisions, errors, and nested modal depth.
- Scope: four Frame Base overlay states; Production remains unchanged.
- Dependencies: `UX-016`.
- Acceptance: each overlay is visibly separate from page content, error meaning
  is clear without coloring structural decorators, and nested layers remain
  distinguishable with their underlying decision context still legible.
- Outcome: complete outer borders and a distinct unified overlay surface are in
  place; awaiting review of geometry and nested depth.

### UX-018 — Establish shared content primitives

- Status: decided
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=content-primitives&font=fira&size=14`
- Area: single-pane, split-pane, scrolling, and long-content surfaces
- Theme: content | visual-system | responsive
- Problem: content stories locally encoded headings, fields, tables, selection,
  loading, unavailable, error, empty, status, and output presentation.
- Desired outcome: one reusable content grammar whose spacing, hierarchy,
  truncation, and non-color state identity survive layout pressure.
- Scope: four Frame Base content stories; Production remains unchanged.
- Dependencies: `UX-017` implementation.
- Acceptance: all states remain distinguishable, columns and selection remain
  stable, long values truncate safely, and stories contain no local styling for
  locked content primitives.
- Outcome: accepted. Semantic widgets and focused render invariants cover the
  general content grammar. The compact fixed-column table remains appropriate
  for popups and simple summaries; richer main-pane tables are split into
  `UX-019`. Scrolling single, split, and stacked stories render from one pane-
  region calculation so content ends before the reserved right scrollbar.

### UX-019 — Establish pane data tables

- Status: accepted
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=pane-data-tables&font=fira&size=14`
- Area: main-pane data tables, selected-row details, and column visibility
- Theme: content | interaction | layout
- Problem: popup-style fixed rows do not cover wide, configurable datasets,
  selected-row detail layouts, or long cell content in main panes.
- Desired outcome: define a pane-table system with schema-owned column policy,
  horizontal panning, selected-only wrapping, optional side details, and a
  restrained selection treatment distinct from search.
- Scope: preview-only Frame Base stories. Production remains unchanged.
- Dependencies: accepted `UX-018` primitives and pane geometry.
- Acceptance: compare minimal, cell-rule, and odd/even treatments; required and
  optional columns remain aligned while panning; only the selected record may
  expand; details track selection without stealing table focus; and the column
  chooser applies or cancels visibility changes transactionally.
- Outcome: four deterministic stories and reusable pane-table state, schema,
  rendering, details composition, and chooser primitives are implemented;
  odd/even decoration is selected for main-pane tables. Cell rules remain
  available for compact tables and occupy one cell without padding. Multi-line
  selection is restricted to odd/even tables, which distinguish odd/even ×
  selected/unselected with four row surfaces. Selected surfaces carry the
  accepted restrained amber tint and accent-colored full-height rail.

### UX-020 — Establish adaptive and minimal layouts

- Status: accepted
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=adaptive-minimal-layouts&font=fira&size=14`
- Area: global shell and compact terminal layouts
- Theme: layout | navigation | responsive
- Problem: the accepted foundation has not yet defined which shell content and
  controls survive or regroup when terminal width and height are constrained.
- Desired outcome: a compact-shell contract that preserves orientation and
  primary navigation without crowding or clipping essential content.
- Scope: preview-only Frame Base normal and 50×16 shell stories. Production
  remains unchanged.
- Dependencies: accepted foundation decisions through `UX-019`.
- Acceptance: the compact shell remains legible, persistent controls retain a
  clear hierarchy, and every hidden, shortened, stacked, or moved element has a
  deterministic width- or height-driven rule.
- Outcome: accepted. Below 72 columns or 20 rows, the context header uses the
  existing `app/environment/server` path grammar; workspace navigation keeps
  numbered Home/Dev/Ops labels while they fit and falls back to numbered
  selection shapes only at narrower widths; contextual hints collapse while
  the workspace and Commands/Help/Quit rows remain. `ContextHeader`,
  `WorkspaceSelector`, `design_lab_compact_shell`, and focused size-boundary
  render tests enforce the contract.

### UX-021 — Inventory user goals and derive workflows

- Status: accepted
- Preview: `http://127.0.0.1:4173/?mode=focus&focus=user-goals-workflows&font=fira&size=14`
- Area: Home, Dev, and Ops product responsibilities
- Theme: product | navigation | workflow
- Problem: the visual foundation is coherent, but the minimum developer and
  DevOps jobs have not yet been ranked into goal-led views and stories.
- Desired outcome: a deliberately small product path that supports essential
  development and operations work while providing a clear first home for the
  GOL-162 metrics view.
- Scope: user-goal inventory, workspace ownership, and the first workflow story;
  metrics acquisition and signal selection follow once view responsibility is
  clear.
- Dependencies: accepted visual foundation through `UX-020`.
- Acceptance: each retained goal has a user, trigger, desired result, workspace
  owner, and minimum data/actions; the first workflow story is explicit enough
  for deterministic preview fixtures.
- Outcome: the first product is an Ops-only explorer. Agents is the initial
  subject; Overview and Metrics are production views; Activity later provides
  Timeline and Journal over an explicit bounded agent selection. Home is
  dropped, and Dev returns only with a rebuilt workflow.

### UX-022 — Promote the foundation into an Ops-first product

- Status: awaiting visual acceptance
- Preview: deterministic Ops agent, fake-OTLP-explorer, and Activity stories
- Area: production shell and Ops explorer
- Theme: product | navigation | content | implementation
- Problem: the accepted components remain preview-only while production still
  renders the obsolete Home/Dev/Ops workbench and visual baseline.
- Desired outcome: one production renderer using the accepted shell, pane,
  table, overlay, scrollbar, and compact-layout rules.
- Scope: Ops-only shell, live Agents Overview, fake OTLP Metrics demo, and
  preview-only Activity contract.
- Dependencies: accepted foundation through `UX-021`.
- Acceptance: `[Ops]` is the only workspace selector; old workflows are not
  reachable; Agents uses live typed data; every invented Metrics series is
  persistently marked `FAKE` and states that no receiver/store was queried;
  Activity is absent from production; all cases survive wide, compact, and
  extreme non-zero sizes.
- Outcome: implemented; awaiting visual acceptance.
  The Columns chooser uses the popup-table selection grammar and exposes
  `↑/↓ Navigate` explicitly.
  Agents now distinguishes exact server-side Dataset filtering from local
  fuzzy Find over loaded rows. Cursor loading reports only loaded count and
  `more available`, and loaded-match All/None actions are explicit. The details
  pane is focusable, scrollable, resizable, and shows structured AgentID plus
  colored agent-type and instance metadata. Auto-refresh state is visible in
  the pane header.
