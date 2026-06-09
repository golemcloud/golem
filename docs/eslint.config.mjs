import nextConfig from "eslint-config-next/core-web-vitals"

export default [
  ...nextConfig,
  {
    files: ["**/_meta.js"],
    rules: {
      "import/no-anonymous-default-export": "off",
    },
  },
]
