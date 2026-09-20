export const externalPackages = (id) =>
  id === "@golemcloud/effect-golem" ||
  id === "@golemcloud/effect-golem/sqlite" ||
  id === "@golemcloud/effect-golem/postgres" ||
  id === "@golemcloud/effect-golem/mysql" ||
  id === "@golemcloud/effect-golem/ignite2" ||
  id === "effect" ||
  id === "effect/unstable/http" ||
  id === "effect/unstable/httpapi" ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:") ||
  id === "agent-guest"
