// Global site-wide constants and external URLs.
// Keep this file small — section-specific copy lives in `homepage.ts`.

export const site = {
  title: "Golem — The durable agent runtime",
  description:
    "Golem is the durable agent runtime that persists state, executes tools transactionally, and enforces every policy. Trust by construction.",
  brand: {
    name: "Golem",
  },
} as const;

export const urls = {
  github: "https://github.com/golemcloud/golem",
  discord: "https://discord.com/invite/UjXeH8uG4x",
  quickstart: "https://learn.golem.cloud/quickstart",
  docs: "https://learn.golem.cloud",
  subscribe: "/subscribe",
  roadmap: "/roadmap",
} as const;
