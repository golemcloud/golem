# TUI DX/UX Review

This document is the operational ledger for iterative TUI design review. It
tracks feedback from deterministic preview stories until each note is accepted,
deferred, or incorporated into an authoritative plan document.

## Review Workflow

1. Start the watched browser gallery with `cargo make tui-preview`.
2. Review the registered real production fixture and the live terminal path.
3. Add the smallest focused lab case set only when the active question cannot
   be evaluated clearly through the production fixture.
4. Record observations under the current focus and codify accepted outcomes in
   a named widget, token, typed model, or tested layout invariant.
5. Delete temporary lab cases after acceptance unless they add unique
   regression coverage, then synchronize the owning design-system document.

`Production` is the only visual style and runtime baseline. The preview host
does not retain candidate, comparison, coverage, or Frame Base machinery.

## Current Focus

- Focus: consolidate and accept stable destination-first Ops (`UX-025`,
  `UX-026`).
- Cases: production-rendered fixtures for every Ops destination at wide,
  medium, narrow, short, and extreme terminal sizes.
- Decision: the subject/view prototype is retired. The accepted Dev shell,
  context header, footer, pane chrome, table language, notices, overlays, and
  color tokens remain the only visual foundation.
- Scope: stable tabs, explicit scope handoff, Dashboard/Explore, grouped and
  exact Oplog, background Stream, Deployment history/diff, and typed
  Environment/Account/Server Configuration.
- Exit: all page/state fixtures are green, the local demo passes in a real
  terminal, and no new page introduces an unreviewed visual language.

## Sources Of Truth

- Locked product behavior and open planning gates belong in
  `product-contract.md`.
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

## Preview Coverage

Record the most recent review status and active note IDs for persistent preview
cases. Temporary lab cases belong here only while their design session is
active.

| Preview case | Last reviewed | Active notes |
|---|---|---|
| `ops-overview` | automated render check; visual review pending | `UX-025` |
| `ops-agents` | automated render check; visual review pending | `UX-025` |
| `ops-agent-detail` | automated render check; visual review pending | `UX-025` |
| `ops-oplog` | automated render check; visual review pending | `UX-025` |
| `ops-stream` | automated render check; visual review pending | `UX-025` |
| `ops-metrics-dashboard` | automated render check; visual review pending | `UX-025` |
| `ops-metrics-explore` | automated render check; visual review pending | `UX-025` |
| `ops-deployments` | automated render check; visual review pending | `UX-025` |
| `ops-configuration` | automated render check; visual review pending | `UX-025` |

## Historical Themed Batches

The entries below record the retired design-lab process and remain historical
context. Their case IDs, Frame Base links, candidate status, and synthetic
renderers are no longer live contracts.

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

- Status: retired after foundation acceptance
- Notes: `UX-003`
- Cases: all six generic design-lab cases
- Validation: every case under Production and Frame Base; preview exporter tests,
  and production wrapper equivalence

### Iterate on Frame Base

- Status: retired after promotion into Production
- Notes: `UX-004`
- Cases: `shell-default`, `split-focus`, `overlay-search`, `overlay-decision`
- Validation: Frame Base structural assertions and all design-lab cases under
  Production and Frame Base

### Compare pane boundaries

- Status: retired after promotion into Production
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
  accepted restrained amber tint and accent-colored full-height rail. The
  later product contract supersedes selected-row expansion for primary resource
  collections: those rows are one line and full values live in Details.

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
  render tests enforce the historical shell. `UX-023` supersedes the numbered
  Home/Dev/Ops navigation with grouped, scalable Dev/Ops navigation while
  retaining the compact-priority principle.

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
  dropped, and Dev returns only with a rebuilt workflow. `UX-023` records that
  rebuilt Dev/Ops contract and supersedes the Ops-only product boundary.

### UX-022 — Promote the foundation into an Ops-first product

- Status: accepted
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
- Outcome: accepted as the visual and Agents interaction baseline; its Ops-only
  product topology is superseded by `UX-023`.
  The Columns chooser uses the popup-table selection grammar and exposes
  `↑/↓ Navigate` explicitly.
  Agents now distinguishes exact server-side Dataset filtering from local
  literal Find over loaded rows. Cursor loading reports only loaded count and
  `more available`, and loaded-match All/None actions are explicit. The details
  pane is focusable, scrollable, resizable, and shows structured AgentID plus
  colored agent-type and instance metadata. Auto-refresh state is visible in
  the pane header.

### UX-023 — Lock the Dev/Ops rebuild contract and readiness

- Status: implemented
- Preview: no new preview; this note governs planning before workflow stories
- Area: product topology, navigation, layout, sessions, Ops resources,
  Activity, Metrics, actions, and implementation order
- Theme: product | navigation | workflow | safety
- Problem: the accepted visual foundation and Agents explorer do not define a
  decision-complete minimal product, while older documents preserve conflicting
  Home, Ops-only, preset, drawer, and observability-backend directions.
- Desired outcome: one authoritative contract states every locked decision,
  every remaining planning gate, and an ordered checkpoint plan with explicit
  acceptance and commit boundaries.
- Scope: `product-contract.md`, `README.md`, `architecture.md`, `ui-system.md`,
  `tasks.md`, `phases.md`, `testing.md`, `dx-ux-review.md`, and `progress.md`.
- Dependencies: accepted visual and Agents baseline through `UX-022` and the
  subsequent Dev/Ops design discussion.
- Acceptance: the listed documents agree on Dev/Ops ownership, hierarchical
  navigation, right/bottom subviews, finite Jobs versus persistent sessions,
  Ops subjects, bounded Activity, provider-independent Metrics, action safety,
  deferred acquisition, readiness gates, and implementation order.
- Outcome: documentation reviewed and the accepted local Dev/read-only Ops
  implementation slice authorized. Implemented in checkpoint `721390b80`;
  later gated work remains out of scope.

### UX-024 — Accept rebuilt local Dev and read-only Ops

- Status: awaiting terminal review
- Preview: Production runtime and the `ops-agents` production-rendered fixture
- Area: Dev workflows, nested terminals, recursive layout, Ops subjects,
  action availability, and context safety
- Theme: workflow | navigation | safety | terminal
- Problem: deterministic tests establish behavior and geometry but cannot
  establish whether the complete interactive flow feels coherent in the real
  terminal.
- Desired outcome: accept or correct the smallest implemented product slice
  before reopening planning for mutations, Activity, or Metrics providers.
- Scope: Plan/Build/Deploy/Clean forms and Jobs; Server and REPL sessions;
  Dev/Ops navigation and layout; Agents and individual Agent; read-only current
  Deployment, Environment, and Server; unavailable production Metrics.
- Dependencies: `UX-023` and implementation checkpoint `721390b80`.
- Acceptance: real terminal review confirms focus, input ownership, pane
  resizing, scrollback, context identity, state continuity, errors, compact
  behavior, and the read-only resource content. The matching dev-release
  playground smoke test is green.
- Outcome: initial terminal review rejected the visual drift. The corrective
  batch keeps the rebuilt behavior but restores `ContextHeader`, `PaneHeader`,
  neutral pane boundaries, shared scrollbars, footer shortcut rows, and
  semantic status markers across Dev and Ops. Acceptance remains pending a new
  terminal review. Selected-target consolidation and cross-Server context-switch
  enforcement remain named implementation follow-ups rather than hidden
  acceptance assumptions.
- Follow-up interaction pass unifies All/Context Commands, makes CLI/TUI input
  ownership visible for every nested terminal, adds exact pane-driven PTY
  resize, content-sized review dialogs with Run first, and a safe/default
  Return plus explicit Force quit dialog. Terminal acceptance remains pending.
- Second interaction pass makes popup notices centered and wrap-aware, adds
  cursor-backed transactional value editing, moves confirmation options to the
  top, keeps Server input TUI-owned on launch, makes ports observation optional,
  exposes a full-rail resize target with an accent grip, and gives Commands
  remembered scope plus deterministic visible match ranking/highlighting.
  Focused automated acceptance is green; real-terminal acceptance remains
  pending.

### UX-025 — Accept stable destination-first Ops

- Status: implemented; awaiting browser and terminal review
- Preview: all production-rendered cases in the Preview Coverage table
- Area: stable Ops navigation, explicit scope handoff, Overview, Agents,
  Oplog, Stream, Metrics, Deployments, and Configuration
- Theme: information architecture | observability | investigation | terminal
- Problem: the earlier subject/view prototype made navigation hard to predict,
  mixed unrelated page languages, and did not provide a credible TUI metrics
  surface.
- Desired outcome: a minimal read-only Ops workspace with stable destinations,
  retained page routes, explicit scope transfer, bounded data access, and
  Grafana-like curated charts without introducing a separate visual system.
- Scope: production ready states for all destinations; Dashboard and Explore;
  grouped and exact Oplog; exact-Agent Stream; deployment history and compare;
  typed Environment, Account, and Server Configuration; Overview routing.
- Dependencies: the accepted shell foundation and local observability backend.
- Acceptance: every destination has deterministic production-render coverage
  at wide, medium, narrow, short, and 1×1 sizes; the remaining exceptional and
  nested-route fixtures are green; the local demo passes a real terminal review.
- Outcome: the ready-state catalogue and size matrix are implemented and
  automated checks are green. Exceptional/nested-route fixtures and manual
  visual acceptance remain open.

### UX-026 — Consolidate Ops hierarchy and interaction

- Status: implemented in slices; awaiting combined visual review
- Preview: production-rendered Ops catalogue and live terminal
- Area: Overview, destination hierarchy, cross-view handoffs, Oplog, Metrics,
  text input, and pointer interaction
- Theme: navigation | content | interaction | visual-system | performance
- Problem: the destination model is sound, but Overview performs expensive
  work for a busy landing page, related views are not visibly connected,
  Oplog remains a bare data projection, page hierarchy/chrome varies, and
  several active inputs and pointer targets do not clearly own interaction.
- Desired outcome: a lightweight routing cockpit, one legible Ops hierarchy,
  visible investigation handoffs, a useful master/details Oplog explorer, and
  consistent cursor and mouse behavior without adding a second visual system.
- Scope: all Ops destinations and the shared search, pane, split, table,
  scrollbar, and chart primitives they compose.
- Dependencies: `UX-025` and the accepted shared shell foundation.
- Acceptance: Overview does not query dashboard metrics merely to render;
  active inputs have a native cursor; destination nesting and pane boundaries
  are consistent; scope transfers are discoverable without Commands; Oplog
  supports readable and forensic navigation; mouse behavior matches keyboard
  ownership; Metrics receives a final terminal-density review.
- Outcome: explicit input ownership and the lightweight Overview cockpit are
  implemented and their focused checks are green. Hierarchy, handoffs, Oplog,
  pointer parity, and final Metrics review remain tracked in `tasks.md`.
