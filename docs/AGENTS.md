# AGENTS.md — Golem Documentation

This is the source for **Golem's official documentation** at [learn.golem.cloud](https://learn.golem.cloud).

It lives under `docs/` in the main `golemcloud/golem` repo. CI (`.github/workflows/docs.yaml`) builds it on docs-only changes and deploys to Vercel.

## Tech Stack

- **Framework**: [Nextra](https://nextra.site/) v4 (docs theme) on top of Next.js 16 (app router)
- **Package manager**: [Bun](https://bun.sh/) (use `bun` for install/run, not `npm`/`yarn`)
- **Styling**: Tailwind CSS, PostCSS
- **Language**: TypeScript, MDX
- **Linting/Formatting**: ESLint + Prettier (with Tailwind plugin); see *Pre-commit Checks* below
- **Syntax highlighting**: Shiki, with custom grammars for `wit` and `rib` languages (see `wit-grammar.json`, `rib-grammar.json`)
- **API docs**: Auto-generated from OpenAPI spec (`openapi/golem-service.yaml`) via `openapi/gen-openapi.ts`
- **How-To Guides**: Auto-generated from the in-tree skill catalog (`../golem-skills/skills`) via `skills/sync-skills.ts`
- **Versioning**: Every page lives under `src/content/<version>/`. The version selector and per-version search are wired up in `src/app/[version]/layout.tsx`. The registry of versions and the default version live in `src/lib/versions.ts`.

## Project Structure

```
src/
  app/
    layout.tsx              # Minimal root layout (html/body/fonts/redirects)
    page.tsx                # Redirects / -> /<DEFAULT_VERSION>
    [version]/
      layout.tsx            # Nextra Layout, selector, banner, per-version search
      [[...mdxPath]]/       # Catch-all that renders MDX under src/content/<version>/
    proxy.ts                # Redirects un-versioned URLs to /<DEFAULT_VERSION>/...
  components/               # React components used in MDX pages, navbar, banner
  content/
    v1.5/                   # MDX pages for the v1.5 release (frozen snapshot)
    next/                   # MDX pages for the unreleased version
                            # Each version mirrors the same directory layout
                            # (concepts/, develop/, rest-api/, how-to-guides/, ...)
  lib/
    versions.ts             # Version registry (slugs, labels, DEFAULT_VERSION)
    version-manifest.ts     # Per-version route manifest (used by the selector)
    releases.tsx            # CLI binary download URLs per release
  styles/                   # Global CSS (Tailwind)
openapi/                    # OpenAPI generator (writes to src/content/<version>/rest-api/)
skills/                     # How-To Guides sync (writes to src/content/<version>/how-to-guides/)
scripts/
  version-tool.ts           # prefix/clone/rename/check for versioned MDX
check-links/                # Link checker (knows about versioned paths + public/)
public/                     # Static assets (images, favicon) — referenced un-prefixed
```

## Common Commands

- `bun install` — Install dependencies
- `bun run dev` — Start dev server (http://localhost:3001)
- `bun run build` — Production build
- `bun run lint` — Lint and auto-fix
- `bun run format` — Format with Prettier
- `bun run fix` — Lint + format together
- `bun run check-links` — Validate links in MDX files
- `bun run generate-local` — Regenerate REST API docs from `../openapi/golem-service.yaml` (the in-tree spec)
- `bun run update-skills` — Sync How-To Guides from the Golem skill catalog on GitHub
- `bun run update-skills-local` — Sync How-To Guides from the in-tree `../golem-skills/skills`

For a typical monorepo workflow, prefer the `cargo make` wrappers (which set up Bun and pin to the in-tree sources):

- `cargo make generate-openapi` — regenerates `openapi/golem-service.yaml` from the Rust services **and** the REST API MDX under `docs/src/content/next/rest-api/`.
- `cargo make generate-docs-openapi` — regenerates only the REST API MDX (skips the service build).
- `cargo make generate-docs-skills` — regenerates How-To Guides under `docs/src/content/next/how-to-guides/` from `golem-skills/skills/`.
- `cargo make check-docs-openapi` / `cargo make check-docs-skills` — CI drift detection (run by `unit-tests-and-checks`).

The auto-generated content always targets the unreleased `next` version. Older versions are frozen snapshots. To target a different version explicitly, pass `--version <slug>` to either generator (e.g., `bun run openapi/gen-openapi.ts --version v1.5`).

## Writing Documentation

### Page structure

- Each page is an `.mdx` file under `src/content/<version>/`. The file path (relative to the version directory) determines the URL.
- Sidebar order and titles are controlled by `_meta.js` files in each directory.
- Use Nextra built-in components (`Callout`, `Tabs`, `Cards`, `Steps`, etc.) — import from `nextra/components`.
- Use the custom `<MultiPlatformCommand>` component for commands that differ by OS/platform.
- Icons come from `lucide-react`.

### Code blocks

- Standard fenced code blocks with language identifiers work out of the box.
- Use `wit` or `rib` as language identifiers for Golem-specific WIT interface definitions and Rib expressions — custom Shiki grammars are configured.

### Adding a new page

1. Pick the target version. New content normally goes under `src/content/next/` (the unreleased version).
2. Create a `.mdx` file under `src/content/<version>/` at the desired path.
3. Add an entry in the corresponding `_meta.js` to set title and sidebar position.
4. All internal links in MDX must be version-prefixed (e.g., `/next/quickstart`). Public-asset links (`/images/...`, `/favicon.ico`) stay un-prefixed.
5. Run `bun run scripts/version-tool.ts check` from `docs/` to verify; it exits non-zero on any unversioned doc link.

### Cutting a new docs version (or backporting fixes to an older one)

See the **`managing-docs-versions`** skill at `.agents/skills/managing-docs-versions/SKILL.md` for the full workflow (promote `next` → `vX.Y`, re-seed `next`, update the registry, regenerate auto-generated content, retire legacy versions, etc.).

### REST API docs

REST API reference pages are auto-generated from the in-tree OpenAPI spec. Do not edit them manually — CI checks for drift via `cargo make check-docs-openapi` and will fail any PR with stale generated MDX.

- To regenerate from the in-tree spec: run `cargo make generate-docs-openapi` (or `bun run generate-local`). The MDX under `src/content/next/rest-api/` will be written and auto-formatted.
- `cargo make generate-openapi` does both: regenerates `openapi/golem-service.yaml` from the running services and then regenerates the MDX from it.

### How-To Guides

How-To Guide pages are auto-generated from the [Golem skill catalog](../golem-skills/skills). Do not edit files under `src/content/next/how-to-guides/` manually — CI checks for drift via `cargo make check-docs-skills` and will fail any PR with stale generated MDX.

- `cargo make generate-docs-skills` regenerates from the in-tree `../golem-skills/skills/` (preferred in the monorepo).
- `bun run update-skills-local` does the same without going through cargo make.
- `bun run update-skills` fetches the latest skills from GitHub instead (useful when working outside the monorepo).

The sync script (`skills/sync-skills.ts`) strips AI-agent frontmatter, converts cross-references between skills into doc links, and generates MDX pages organized by category (General, Rust, TypeScript, Scala, MoonBit). Set `GITHUB_TOKEN` env var to avoid GitHub API rate limits when using `update-skills`.

## Pre-commit Checks

When this lived in its own repo, [Lefthook](https://github.com/evilmartians/lefthook) wired up automatic pre-commit hooks. In the monorepo we no longer auto-install them (Lefthook lives at the git root, which would conflict with the rest of the golem repo's hooks).

CI still enforces all of the same checks on every PR via `.github/workflows/docs.yaml` → `bun run build:check`, which runs:

1. ESLint (`bun run lint:check`)
2. Prettier (`bun run format:check`)
3. TypeScript type checking (`bun run typecheck`)
4. Link validation (`bun run check-links`)

Run them locally with `bun run build:check`, or auto-fix lint/format issues with `bun run fix`.
