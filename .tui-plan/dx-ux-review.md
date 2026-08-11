# TUI DX/UX Review

This document is the operational ledger for iterative TUI design review. It
tracks feedback from deterministic preview stories until each note is accepted,
deferred, or incorporated into an authoritative plan document.

## Review Workflow

1. Start the watched browser gallery with `cargo make tui-preview`.
2. Review one story at a time in the default **Review Production** view.
3. Copy the story link and record free-form observations against that link.
4. Normalize each observation into a decision-ready note in the ledger below.
5. Add a deterministic story first when the reported state is not represented.
6. Group ready notes into a small themed batch and approve that batch explicitly
   before implementation.
7. Review the implementation through the same stable links, then accept, revise,
   or defer each note.

Use **Compare variants** only for visual-system choices. `Production` is the
runtime baseline; accepting a preview variant means deliberately changing
`Production`, not exposing a user-selectable theme. Use **Variant coverage** to
scan every story for a selected visual direction.

## Sources Of Truth

- Durable product, visual, and interaction rules belong in `ui-system.md`.
- Approved executable work belongs in `tasks.md`.
- Completed batches and validation history belong in `progress.md`.
- Rendering and scenario validation policy belongs in `testing.md`.
- This file indexes active review state and links to those outcomes instead of
  duplicating their final wording.

Synchronize the relevant source-of-truth document when a note becomes
`approved` or `accepted`. Do not treat conversation history or preview-only
variants as authoritative design state.

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
- `accepted`: reviewed and synchronized into the appropriate sources of truth.
- `deferred`: intentionally postponed with a recorded reason or dependency.

## Story Coverage

Record the most recent review status and active note IDs for each preview case.
The preview case ID, rather than its underlying fixture name, is the stable
identity; this keeps compact and wide cases independently reviewable.

| Preview case | Last reviewed | Active notes |
|---|---|---|
| `home-idle` | 2026-08-06 | `UX-001` |
| `home-active` | 2026-08-06 | `UX-001` |
| `dev-running` | — | — |
| `dev-completed` | — | — |
| `dev-failed` | — | — |
| `dev-server-drawer` | — | — |
| `dev-layout-left` | — | — |
| `dev-layout-top` | — | — |
| `dev-layout-bottom` | — | — |
| `ops-list` | — | — |
| `ops-details` | — | — |
| `ops-loading` | — | — |
| `ops-error` | — | — |
| `agent-inspect` | — | — |
| `palette` | — | — |
| `help` | — | — |
| `context-picker` | — | — |
| `loading` | — | — |
| `confirmation` | — | — |
| `home-compact` | 2026-08-06 | `UX-001` |
| `ops-wide` | — | — |

## Themed Batches

Before implementation, record the batch name, included note IDs, affected
stories, intended outcome, and validation. Mark the batch approved only after
explicit review.

### Remove the Home logo background

- Status: implemented; awaiting review
- Notes: `UX-001`
- Stories: `home-idle`, `home-active`, `home-compact`
- Validation: focused Home render test and all deterministic preview cases

## Active Notes

### UX-001 — Remove the Home logo background

- Status: implemented
- Preview: `http://127.0.0.1:4173/?mode=review&case=home-idle&variant=Production&font=fira&size=14`
- Area: Home workspace background
- Theme: visual-system
- Problem: the large Braille Golem logo depends on platform glyph fallback and
  garbles browser previews, making the surrounding UI harder to evaluate.
- Desired outcome: Home uses the normal surface background without decorative
  logo glyphs.
- Scope: `home-idle`, `home-active`, `home-compact`, and Production rendering.
- Dependencies: future logo placement is a separate deferred decision.
- Acceptance: no Braille logo appears behind Home content at standard, compact,
  or wide terminal sizes.
- Outcome: implementation complete; awaiting visual acceptance.
