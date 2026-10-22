---
name: compiling-homepage-snippets
description: Writes, ports and compile-checks the six-language per-feature code snippets on the golem.cloud home page (website/snippets). Use when adding, editing or porting a home-page code snippet, changing a chapter bullet, or checking that the snippets still compile against the in-tree SDKs.
---

# Compiling the home-page code snippets

The home page (`website/`, Astro) has three chapters, Agents, Tools and Artifacts. Each has a list of points and a code figure with a language switcher (TypeScript, Effect, Rust, Scala, MoonBit, Go). Each chapter's first point, **Overview**, is selected at rest and shows the **overview** snippet; pointing at, focusing or clicking another point swaps in **that feature's own snippet**, which stays until another point is picked. Escape restores the overview. A small chevron at the right of each title marks the points as selectable. While a snippet shows, its marked lines are highlighted. Below 1100 px each point except Overview gets a "Show code" button that opens its snippet under it.

The snippets are marketing, not documentation. Their compile harnesses are **local and uncommitted**, and some drift is expected. This skill records how to rebuild those harnesses and re-check the snippets after an edit.

## Where things live

- **Display code (committed):** `website/snippets/<chapter>/<feature>/<language>/<display-filename>`. The file name is shown in the figure header. A file directly in `<chapter>/<feature>/` (the Tools `publish` `golem.yaml`) is shared by every language. The feature folders match the point ids in `website/src/data/homepage.ts`, including `overview`.
- **Highlights:** a line ending in ` //@` (` #@` in YAML) is highlighted while its snippet shows. The renderer and every harness strip the marker, so it never counts toward the width limit.
- **Data:** `homepage.ts` builds each chapter's `snippets` map from the folder layout (`snippets("<chapter>")`). A chapter's `missing` map gives the note shown in place of a feature a language lacks (Agents: `sqlite`). The build fails if a feature has neither a snippet nor a note for some language.
- **Rendering:** `website/src/components/Chapter.astro` stacks every feature's snippet for a language in one grid cell (inactive ones are `visibility: hidden`), so the figure keeps the height of the tallest and swapping moves nothing. It strips highlight markers and adds a `hl` class to marked lines. Its script handles the point selection, the Escape reset, the language switch (the chosen chapter and those below it) and the narrow-screen "Show code" toggles, which clone the figure's snippet under the bullet.
- **Harnesses (local only):** `.amp/in/snippets/<language>/`, holding `check.sh`, `neg.sh`, preludes and `PROCESS.md`. `.amp/in/` is in the git exclude file. Never commit them.
- **Per-language rebuild notes:** `reference/<language>.md` in this skill: prerequisites and versions, harness layout, how display files are spliced into compilable sources, invented libraries, stubs, negative controls and known limits.

## Snippet rules

1. **One idea per snippet.** Each feature snippet makes its point's claims concrete; the overview backs the chapter's Overview point. Keep the same agent, tool, method and config names across languages (`SupportAgent`, tool `refund`, method `issueRefund`, secret key `stripeKey`, tool `order-status`) so the shared YAML stays true.
2. **Highlights.** Mark few lines: the ones that prove each claim in the point's body, preferably the Golem call or declaration rather than syntax. A comment may be marked when it carries a claim no code in the file can show (the Secrets grant and binding claims). Every language marks the counterparts of the TypeScript marks.
3. **Height.** At most 31 displayed lines per snippet, ideally 30; the tallest snippet in a chapter sets that chapter's figure height.
4. **Width.** At most 68 characters per displayed line, so no code wraps at 1100–1440 px. Go uses 4 spaces in place of tabs.
5. **Comments.** Only where they carry the point; each on its own line directly above the code it describes, a complete clause ending with a colon. Keep the TypeScript wording identical in every language, changing only comment syntax and identifier case. The one exception: `agents/team` says `a typed client generated for a Rust ResearchAgent:`, and the Rust port names Go instead.
6. **Precision.** No puffery. Exactly-once claims stay conditional on the external service honouring idempotency keys. Never say "deny by default", "stopped", "as if nothing happened" or "variables persist". Live files are served once the writing call completes, not at once.
7. **Libraries.** Use the real SDK for every Golem feature. Embedded SQLite exists only for TypeScript, Effect and Rust; Scala, MoonBit and Go have no `agents/sqlite` file and show the note instead. Where a language has no real library for something else a snippet needs, such as a model client, the owner's rule is to invent the most plausible library, preferring a real third-party library that compiles for the target. Implement the invented library in the harness as a small, type-correct module that fails loudly at runtime. The current choices are recorded in the references.
8. **Rust errors.** No `.unwrap()` in displayed Rust: functions and agent methods return `Result` and propagate with `?`.
9. **SDK gaps.** When the natural code for a feature does not work in an SDK (for example passing a config secret through a tool client), show the natural code, make it compile with a labelled harness shim, and record the SDK bug in that language's reference.

## Re-checking after an edit

Run every affected language. Each script reads the committed display files directly, so the page and the checked code stay identical byte for byte.

```shell
.amp/in/snippets/ts/check.sh        # tsc against sdks/ts dist; shared publish YAML against the 1.6.0 schema
.amp/in/snippets/effect/check.sh    # tsc against sdks/effect
.amp/in/snippets/rust/check.sh      # cargo check/build --target wasm32-wasip2 + wasm-tools validate + native registration test
.amp/in/snippets/scala/check.sh     # sbt fastLinkJS with the Golem sbt plugin
.amp/in/snippets/moonbit/check.sh   # golem_sdk_tools codegen + moon build --target wasm + component validate
.amp/in/snippets/go/check.sh        # go build/vet for wasip1 + componentize-go components + wasm-tools validate
```

Each prints `ALL_OK` on success. Run `neg.sh` after changing a harness: it plants one wrong Golem API use per snippet and requires each to fail.

If `.amp/in/snippets/<language>/` is missing on a fresh machine, rebuild it from `reference/<language>.md`. Start from the TypeScript harness: it is the smallest, and the other languages port its logic.

## After the code compiles

1. Restart or reload the dev server (`cd website && npx astro dev --port 4321`), then load `http://localhost:4321/`.
2. Run browser QA at 1100, 1280 and 1440 px and at 390 px, for every language you touched. Check that:
   - no code line wraps at desktop widths, and every point body is exactly three lines;
   - each point shows its own snippet and filename, and the figure height never changes while swapping;
   - every snippet except a note has highlighted lines, and the Overview point and Escape restore the overview;
   - on narrow screens, "Show code" opens the right snippet in the current language and nothing overflows.
3. Inspect screenshots of the overview and a few selected features in each chapter before reporting.

## Known limits

Compilation is the only evidence. No snippet has been deployed or run. The references list what each language leaves unproven, for example generated clients compiled against stubs, invented libraries that fail at runtime, and untested webhook delivery, permission-card lifetimes and idempotency behaviour.
