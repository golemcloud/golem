## Getting Started

1. [Install Bun](https://bun.sh/docs/installation)
2. Install dependencies

   `bun install`

3. Run the development server

   `bun run dev`

4. Open http://localhost:3001 with your browser to see the result.

## Development Guide

This docs site is built with [Nextra](https://nextra.site/).

## Adding a new page

Each documentation page is created from a .mdx file. To add a new page, create a new .mdx file in the `src/pages` directory. The file name will be the URL path.

For example, `src/pages/docs/getting-started.mdx` will be available at `/docs/getting-started`.

### Changing page metadata

To change the page title or its position in the left-hand sidebar, create a `_meta.json` file in the same directory as the .mdx file.

[Read more about Nextra's \_meta.json files](https://nextra.site/docs/guide/organize-files#_metajson).

[Read more about Nextra's docs theme](https://nextra.site/docs/docs-theme).

If we had a page `/src/pages/docs/getting-started.mdx`, we could add a `_meta.json` file to change the title and make it the first entry in the sidebar.

```json
{
  "getting-started": {
    "title": "Getting Started - Overview"
  },
  "other-page": "Other Page"
}
```

Note how the key in the JSON object matches the file name. This is how Nextra knows which page to apply the metadata to.

> If the value is a JSON object, you can set multiple parameters. If it's a string, it will just change the title.
