---
name: managing-docs-versions
description: Cutting a new docs version, promoting next to a release, and managing versioned documentation content under docs/src/content/. Use when releasing a Golem version, backporting docs fixes, renaming a docs version, or changing the version selector.
---

# Managing Documentation Versions

Golem documentation is stored under `docs/src/content/<version>/`. Write new material in `next`; released directories are frozen snapshots.

## Current wiring

- `docs/src/lib/versions.ts`: `VERSIONS`, lifecycle status, and `DEFAULT_VERSION`.
- `docs/src/app/[version]/page.tsx`: version index (`/<version>`).
- `docs/src/app/[version]/[...mdxPath]/page.tsx`: non-index pages (`/<version>/...`). These are deliberately split; there is no optional-catch-all route.
- `docs/src/app/[version]/layout.tsx`: version-aware Nextra layout, selector, banner, and Pagefind filter.
- `docs/src/app/page.tsx`: root redirect.
- `docs/src/proxy.ts`: redirects eligible unversioned routes to the default version.
- `docs/src/lib/version-manifest.ts` and `docs/src/components/version-selector.tsx`: same-page/nearest-parent switching between versions.
- `docs/scripts/version-tool.ts`: `prefix`, `clone`, `rename`, and `check`.
- `docs/next.config.mjs`: Next configuration and the place to add explicit redirects.

## Link rules and checker scope

In prose MDX, use version-prefixed documentation links such as `/next/quickstart` or `/v1.5/quickstart`. Keep public assets such as `/images/foo.png` unprefixed.

Run from `docs/`:

```shell
bun run scripts/version-tool.ts check [<registered-version>]
```

Do not overstate this check. It checks registered version prefixes from `VERSIONS` and only the supported prose forms:

- Markdown destinations: `](/path)` and `](/path "title")`
- quoted JSX/HTML `href` and `to` attributes

It skips fenced code, public top-level paths, already registered version prefixes, relative links, and unsupported forms such as `href={...}`. It does not prove that a destination exists, that a cross-version link is desirable, or that an unregistered new slug is valid. `check <slug>` selects a content directory but does not register that slug; update `versions.ts` before relying on it.

## Cut a release (`next` to `vX.Y`)

Run version-tool commands from `docs/`.

1. Move `next` and rewrite links inside the moved tree:

   ```shell
   bun run scripts/version-tool.ts rename next vX.Y
   ```

2. Clone the release back to `next`, rewriting links in the clone:

   ```shell
   bun run scripts/version-tool.ts clone vX.Y next
   ```

3. Update `docs/src/lib/versions.ts`: add `vX.Y` as `current`, demote the previous current version to `legacy`, set `DEFAULT_VERSION`, and retain `next` as `unreleased`.

4. Regenerate the live sources of truth into `next` from the repository root:

   ```shell
   cargo make generate-docs-openapi
   cargo make generate-docs-skills
   ```

   The How-To generator replaces and generates **all** of these outputs:
   - `docs/src/content/next/how-to-guides.mdx`
   - skill pages under `docs/src/content/next/how-to-guides/<category>/`
   - category landing pages `docs/src/content/next/how-to-guides/<category>.mdx`
   - top-level and per-category `_meta.js` files

5. Inspect the diff, especially the frozen release versus regenerated `next`, then verify:

   ```shell
   # from docs/
   bun run scripts/version-tool.ts check
   bun run build:check
   bun run build

   # from the repository root
   cargo make check-docs-openapi
   cargo make check-docs-skills
   ```

6. Smoke-test `/`, both version index routes, nested pages, version switching, and the `next`/legacy banners.

### Verify Pagefind without hardcoded filenames or slugs

`bun run build` runs Pagefind as `postbuild` and writes `docs/public/_pagefind`. Do not depend on compressed internal filenames, `strings`, a fixed filter count, or a hardcoded version list. Derive expected slugs from `VERSIONS`, then query the generated index through Pagefind's JavaScript API (or run the site and use the search UI) and verify that each registered version can be supplied as the `version` filter and returns results for known text in that version. Also confirm the build log contains no Pagefind errors and that `public/_pagefind/pagefind.js` and `pagefind-entry.json` exist.

## Backport to a released version

Edit the frozen MDX directly, preserve that version's link prefix, and run:

```shell
cd docs
bun run scripts/version-tool.ts check vX.Y
bun run build:check
```

Generators accept `--version vX.Y`, but use that only when today's source inputs genuinely represent that release:

```shell
bun run openapi/gen-openapi.ts --version vX.Y
bun run skills/sync-skills.ts --local .. --version vX.Y
```

## Rename, add, or retire a version

- Rename content with `bun run scripts/version-tool.ts rename <old> <new>`, then update `versions.ts`. The tool rewrites links only in the moved tree.
- Add a page under `src/content/<version>/`, register it in the nearest `_meta.js`, and use that version's prefix.
- To retire content, remove its directory and registry entry. **If old URLs should redirect, add an explicit redirect in `docs/next.config.mjs`; removing content or changing the registry does not create one.** Check redirect ordering alongside `src/proxy.ts` and test both index and nested old URLs.

## Editing cautions

- Do not hand-edit generated `next/rest-api/` or any generated How-To output. Change its source and regenerate.
- `prefix` is an initial-migration helper, not routine formatting. `clone` and `rename` reject an existing destination.
- Public assets remain unprefixed. Prefer same-version documentation links; the selector handles navigation across snapshots.
- If development state appears stale, clearing `.next` may help, but do not claim that production builds are categorically unaffected by development-router or bundler problems. Verify the production build.
