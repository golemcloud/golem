import nextra from "nextra"

const withNextra = nextra({
  search: {
    codeblocks: false,
  },
  mdxOptions: {
    rehypePrettyCodeOptions: {
      defaultLang: "text",
    },
  },
})

export default withNextra({
  reactStrictMode: true,
})
