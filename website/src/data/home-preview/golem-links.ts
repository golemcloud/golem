const publishedDocs = "https://learn.golem.cloud/v1.6";

export const golemLinks = {
  docs: "https://learn.golem.cloud",
  quickstart: `${publishedDocs}/quickstart`,
  primitives: `${publishedDocs}/concepts/agents`,
  durability: `${publishedDocs}/concepts/durable-execution`,
  permissions: `${publishedDocs}/concepts/permissions`,
  agents: `${publishedDocs}/concepts/tools`,
  apps: `${publishedDocs}/concepts/artifacts`,
  distributed: `${publishedDocs}/develop/rpc`,
  capabilities: `${publishedDocs}/develop`,
  sdk: `${publishedDocs}/develop/setup`,
  packages: `${publishedDocs}/js-apis`,
  selfHost: `${publishedDocs}/deploy`,
  cloud: `${publishedDocs}/deploy/golem-cloud`,
  languages: {
    TypeScript: `${publishedDocs}/how-to-guides/ts`,
    Effect: `${publishedDocs}/how-to-guides/effect`,
    Rust: `${publishedDocs}/how-to-guides/rust`,
    Scala: `${publishedDocs}/how-to-guides/scala`,
    MoonBit: `${publishedDocs}/how-to-guides/moonbit`,
    Go: `${publishedDocs}/how-to-guides/go`,
    Python: `${publishedDocs}/develop/setup`,
  } as Record<string, string>,
};
