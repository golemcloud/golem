// Global site-wide constants and external URLs.
// Keep this file small — section-specific copy lives in `homepage.ts`.

export const site = {
  title: "Golem — Agents, tools, and artifacts on one durable runtime",
  description:
    "Golem is the durable runtime for agents, their tools, and their artifacts — state persists, every tool call executes exactly once, and the host enforces every policy. Open source.",
  brand: {
    name: "Golem",
  },
} as const;

export const urls = {
  github: "https://github.com/golemcloud/golem",
  discord: "https://discord.com/invite/UjXeH8uG4x",
  quickstart: "https://learn.golem.cloud/quickstart",
  docs: "https://learn.golem.cloud",
  releases: "https://github.com/golemcloud/golem/releases",
} as const;
