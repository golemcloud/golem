# Golem website

Static [Astro](https://astro.build) site for `golem.cloud`.

## Local development

Requires Node ≥ 22.12.0.

```sh
npm install
npm run dev        # http://localhost:4321
```

## Commands

| Command             | Action                                           |
| :------------------ | :----------------------------------------------- |
| `npm run dev`       | Start the dev server at `localhost:4321`         |
| `npm run build`     | Build the static site and the Pagefind index    |
| `npm run preview`   | Serve the production build locally               |
| `npm run format`    | Format source with Prettier                      |
| `npm run lint:md`   | Lint blog/research markdown with markdownlint    |
| `npm run lint:md:fix` | Auto-fix markdown lint issues                  |

## Project layout

```text
src/
├── components/   Astro components used across pages
├── content/      Content collections (blog posts as .md)
├── data/         Site copy — single source of truth for visitor-facing text
├── layouts/      Page shells
├── pages/        Routes (one file per URL)
└── styles/       Global CSS
public/           Static assets served at the site root (favicons, logos, photos)
```

Editorial copy lives in `src/data/` (not in `.astro` files) — that's where to edit visible text.

## Deployment (Netlify)

The site is configured for Netlify deployment via [`netlify.toml`](./netlify.toml):

- **Build command:** `npm run build`
- **Publish directory:** `dist`
- **Node version:** pinned to `22.12.0`
- **Redirect:** `/post/*` → `/blog/*` (301), preserving inbound links to the
  old `golem.cloud/post/<slug>` blog URLs.

To deploy: connect the repo to a Netlify site. No additional configuration in the Netlify UI should be necessary.

## Environment variables

None are currently required for the build. The site has no runtime backend,
no third-party integrations, and the canonical site URL (`https://golem.cloud`)
is hardcoded in `astro.config.mjs`.

If you need local-only overrides, create a `.env.local` file (gitignored)
following [Astro's env conventions](https://docs.astro.build/en/guides/environment-variables/) —
variables prefixed `PUBLIC_` are exposed to client code; others stay
server-side at build time.
