# AGENTS.md — Golem Documentation

This is the source for **Golem's official documentation** at [learn.golem.cloud](https://learn.golem.cloud).

## Tech Stack

- **Framework**: [Nextra](https://nextra.site/) v2 (docs theme) on top of Next.js 14
- **Package manager**: [Bun](https://bun.sh/) (use `bun` for install/run, not `npm`/`yarn`)
- **Styling**: Tailwind CSS, PostCSS
- **Language**: TypeScript, MDX
- **Linting/Formatting**: ESLint + Prettier (with Tailwind plugin), enforced via [Lefthook](https://github.com/evilmartians/lefthook) pre-commit hooks
- **Syntax highlighting**: Shiki, with custom grammars for `wit` and `rib` languages (see `wit-grammar.json`, `rib-grammar.json`)
- **API docs**: Auto-generated from OpenAPI spec (`openapi/golem-service.yaml`) via `openapi/gen-openapi.ts`
- **How-To Guides**: Auto-generated from the [Golem skill catalog](https://github.com/golemcloud/golem/tree/main/golem-skills/skills) via `skills/sync-skills.ts`

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
- `bun run update-skills` — Sync How-To Guides from the Golem skill catalog (fetches from GitHub)
- `bun run update-skills-local -- --local <path>` — Sync How-To Guides from a local golem repo checkout

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

- To update from a **local copy** of the spec: copy the YAML from the golem repo (`../golem/openapi/golem-service.yaml`) into `openapi/golem-service.yaml`, then run `bun run generate-local`.
- `bun run generate-prod` and `bun run generate-dev` fetch the OpenAPI spec from the respective deployed environments — they do **not** use the local YAML file.

The generated MDX files under `src/pages/rest-api/` will be updated and auto-formatted.

### How-To Guides

How-To Guide pages are auto-generated from the [Golem skill catalog](https://github.com/golemcloud/golem/tree/main/golem-skills/skills). Do not edit files under `src/pages/how-to-guides/` manually.

- `bun run update-skills` fetches the latest skills from GitHub.
- `bun run update-skills-local -- --local <path>` reads from a local checkout of the golem repo.

The sync script (`skills/sync-skills.ts`) strips AI-agent frontmatter, converts cross-references between skills into doc links, and generates MDX pages organized by category (General, Rust, TypeScript, Scala). Set `GITHUB_TOKEN` env var to avoid GitHub API rate limits when fetching remotely.

## Pre-commit Checks

Lefthook runs the following on staged files before each commit:

1. ESLint (fix mode)
2. Prettier (write mode)
3. TypeScript type checking (`tsc`)
4. Link validation (`check-links`)

Ensure all four pass before committing. Run `bun run fix` to auto-fix lint and format issues.
