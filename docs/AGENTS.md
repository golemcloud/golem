# AGENTS.md — Golem Documentation

This is the source for **Golem's official documentation** at [learn.golem.cloud](https://learn.golem.cloud).

It lives under `docs/` in the main `golemcloud/golem` repo. CI (`.github/workflows/docs.yaml`) builds it on docs-only changes and deploys to Vercel.

## Tech Stack

- **Framework**: [Nextra](https://nextra.site/) v2 (docs theme) on top of Next.js 14
- **Package manager**: [Bun](https://bun.sh/) (use `bun` for install/run, not `npm`/`yarn`)
- **Styling**: Tailwind CSS, PostCSS
- **Language**: TypeScript, MDX
- **Linting/Formatting**: ESLint + Prettier (with Tailwind plugin); see *Pre-commit Checks* below
- **Syntax highlighting**: Shiki, with custom grammars for `wit` and `rib` languages (see `wit-grammar.json`, `rib-grammar.json`)
- **API docs**: Auto-generated from OpenAPI spec (`openapi/golem-service.yaml`) via `openapi/gen-openapi.ts`
- **How-To Guides**: Auto-generated from the in-tree skill catalog (`../golem-skills/skills`) via `skills/sync-skills.ts`

## Project Structure

```
src/
  pages/         # MDX documentation pages (file-based routing)
  components/    # React components used in MDX pages
  lib/           # Shared utilities (e.g., release version info)
  styles/        # Global CSS (Tailwind)
  context/       # React context providers
openapi/         # OpenAPI spec and code generation script
skills/          # How-To Guides sync script
check-links/     # Link-checking tool for MDX files
public/          # Static assets (images, favicon)
theme.config.tsx # Nextra docs theme configuration
```

## Common Commands

- `bun install` — Install dependencies
- `bun run dev` — Start dev server (http://localhost:3001)
- `bun run build` — Production build
- `bun run lint` — Lint and auto-fix
- `bun run format` — Format with Prettier
- `bun run fix` — Lint + format together
- `bun run check-links` — Validate links in MDX files
- `bun run generate-prod` — Regenerate REST API docs from OpenAPI spec
- `bun run update-skills` — Sync How-To Guides from the Golem skill catalog on GitHub
- `bun run update-skills-local -- --local ..` — Sync How-To Guides from the in-tree `../golem-skills/skills`

## Writing Documentation

### Page structure

- Each page is an `.mdx` file under `src/pages/`. The file path determines the URL.
- Sidebar order and titles are controlled by `_meta.json` files in each directory.
- Use Nextra built-in components (`Callout`, `Tabs`, `Cards`, `Steps`, etc.) — import from `nextra/components`.
- Use the custom `<MultiPlatformCommand>` component for commands that differ by OS/platform.
- Icons come from `lucide-react`.

### Code blocks

- Standard fenced code blocks with language identifiers work out of the box.
- Use `wit` or `rib` as language identifiers for Golem-specific WIT interface definitions and Rib expressions — custom Shiki grammars are configured.

### Adding a new page

1. Create a `.mdx` file under `src/pages/` at the desired path.
2. Add an entry in the corresponding `_meta.json` to set title and sidebar position.

### REST API docs

REST API reference pages are auto-generated. Do not edit them manually.

- To update from a **local copy** of the spec: copy the YAML from the in-tree golem services (`../openapi/golem-service.yaml`) into `openapi/golem-service.yaml`, then run `bun run generate-local`.
- `bun run generate-prod` and `bun run generate-dev` fetch the OpenAPI spec from the respective deployed environments — they do **not** use the local YAML file.

The generated MDX files under `src/pages/rest-api/` will be updated and auto-formatted.

### How-To Guides

How-To Guide pages are auto-generated from the [Golem skill catalog](../golem-skills/skills). Do not edit files under `src/pages/how-to-guides/` manually.

- `bun run update-skills` fetches the latest skills from GitHub.
- `bun run update-skills-local -- --local ..` reads from the in-tree `../golem-skills/`.

The sync script (`skills/sync-skills.ts`) strips AI-agent frontmatter, converts cross-references between skills into doc links, and generates MDX pages organized by category (General, Rust, TypeScript, Scala). Set `GITHUB_TOKEN` env var to avoid GitHub API rate limits when fetching remotely.

## Pre-commit Checks

When this lived in its own repo, [Lefthook](https://github.com/evilmartians/lefthook) wired up automatic pre-commit hooks. In the monorepo we no longer auto-install them (Lefthook lives at the git root, which would conflict with the rest of the golem repo's hooks).

CI still enforces all of the same checks on every PR via `.github/workflows/docs.yaml` → `bun run build:check`, which runs:

1. ESLint (`bun run lint:check`)
2. Prettier (`bun run format:check`)
3. TypeScript type checking (`bun run typecheck`)
4. Link validation (`bun run check-links`)

Run them locally with `bun run build:check`, or auto-fix lint/format issues with `bun run fix`.
