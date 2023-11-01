## Getting Started

1. [Install Bun](https://bun.sh/docs/installation)
2. Install dependencies

   `bun install`

3. Run the development server

   `bun run dev`

4. Open http://localhost:3000 with your browser to see the result.

## Developing Guide

This docs site is built with [Nextra](https://nextra.site/)

## Adding a new page

Each docs page is created from a .mdx file. To add a new page, create a new .mdx file in the `pages` directory. The file name will be the URL path.

For example, `pages/docs/getting-started.mdx` will be available at `/docs/getting-started`.

### Changing page metadata

In order to change the page's title or placement on the left hand sidebar, you need to create a `_meta.json` file in the same directory as the .mdx file.

[Read more about Nextra's \_meta.json files](https://nextra.site/docs/guide/organize-files#_metajson).

[Read more about Nextra's docs theme](https://nextra.site/docs/docs-theme).

If we had a page `/pages/docs/getting-started.mdx`, we could add a `_meta.json` file to change the title and make it the first entry in the sidebar.

```json
{
  "getting-started": {
    "title": "Getting Started - Overview"
  },
  "other-page": "Other Page"
}
```

Note how the key in the json object matches the file name. This is how Nextra knows which page to apply the metadata to.

> If the value is a json object, you can set multiple parameters. If it's a string, it will just change the title.
