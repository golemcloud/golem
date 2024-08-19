const shiki = require("shiki")
const nextra = require("nextra")

const withNextra = nextra({
  theme: "nextra-theme-docs",
  themeConfig: "./theme.config.tsx",
  mdxOptions: {
    rehypePrettyCodeOptions: {
      getHighlighter: options =>
        shiki.getHighlighter({
          ...options,
          langs: [
            ...shiki.BUNDLED_LANGUAGES,
            {
              id: "wit",
              scopeName: "source.wit",
              aliases: [""], // Along with id, aliases will be included in the allowed names you can use when writing markdown.
              // this is relative path from node_modules/shiki/index.js
              path: "../../wit-grammar.json",
            },
          ],
        }),
    },
  },
})

module.exports = withNextra({
  reactStrictMode: true,
  experimental: {
    scrollRestoration: true,
  },
})
