import { defineConfig } from "vitest/config"
import { fileURLToPath } from "node:url"
import { dirname, resolve } from "node:path"

const here = dirname(fileURLToPath(import.meta.url))
const mockDir = resolve(here, "test/mocks")

/**
 * The Golem runtime exposes a number of host modules with WIT-style
 * specifiers like `golem:agent/host@1.5.0` and `wasi:io/streams@0.2.3`.
 * These are not resolvable by node, so for tests we alias every such
 * specifier to a hand-written mock under `test/mocks/`.
 */
const golemAliases = [
  { find: "golem:agent/host@1.5.0", replacement: resolve(mockDir, "golem-agent-host.ts") },
  { find: "golem:agent/common@1.5.0", replacement: resolve(mockDir, "golem-agent-common.ts") },
  { find: "golem:core/types@1.5.0", replacement: resolve(mockDir, "golem-core-types.ts") },
  { find: "golem:api/host@1.5.0", replacement: resolve(mockDir, "golem-api-host.ts") },
  { find: "golem:api/oplog@1.5.0", replacement: resolve(mockDir, "golem-api-oplog.ts") },
  { find: "golem:api/retry@1.5.0", replacement: resolve(mockDir, "golem-api-retry.ts") },
  {
    find: "golem:durability/durability@1.5.0",
    replacement: resolve(mockDir, "golem-durability.ts"),
  },
  { find: "golem:quota/types@1.5.0", replacement: resolve(mockDir, "golem-quota-types.ts") },
  { find: "wasi:cli/environment@0.2.3", replacement: resolve(mockDir, "wasi-cli-environment.ts") },
  { find: "wasi:logging/logging", replacement: resolve(mockDir, "wasi-logging.ts") },
  { find: "golem:api/context@1.5.0", replacement: resolve(mockDir, "golem-api-context.ts") },
  { find: "node:sqlite", replacement: resolve(mockDir, "node-sqlite.ts") },
  {
    find: "golem:rdbms/postgres@1.5.0",
    replacement: resolve(mockDir, "golem-rdbms-postgres.ts"),
  },
  {
    find: "golem:rdbms/mysql@1.5.0",
    replacement: resolve(mockDir, "golem-rdbms-mysql.ts"),
  },
  {
    find: "golem:rdbms/ignite2@1.5.0",
    replacement: resolve(mockDir, "golem-rdbms-ignite2.ts"),
  },
  {
    find: "golem:rdbms/types@1.5.0",
    replacement: resolve(mockDir, "golem-rdbms-types.ts"),
  },
  {
    find: "golem:websocket/client@1.5.0",
    replacement: resolve(mockDir, "golem-websocket-client.ts"),
  },
  {
    find: "wasi:io/streams@0.2.3",
    replacement: resolve(mockDir, "wasi-io-streams.ts"),
  },
  {
    find: "wasi:keyvalue/types@0.1.0",
    replacement: resolve(mockDir, "wasi-keyvalue-types.ts"),
  },
  {
    find: "wasi:keyvalue/eventual@0.1.0",
    replacement: resolve(mockDir, "wasi-keyvalue-eventual.ts"),
  },
  {
    find: "wasi:keyvalue/eventual-batch@0.1.0",
    replacement: resolve(mockDir, "wasi-keyvalue-eventual-batch.ts"),
  },
  {
    find: "wasi:keyvalue/wasi-keyvalue-error@0.1.0",
    replacement: resolve(mockDir, "wasi-keyvalue-error.ts"),
  },
  {
    find: "wasi:blobstore/types",
    replacement: resolve(mockDir, "wasi-blobstore-types.ts"),
  },
  {
    find: "wasi:blobstore/container",
    replacement: resolve(mockDir, "wasi-blobstore-container.ts"),
  },
  {
    find: "wasi:blobstore/blobstore",
    replacement: resolve(mockDir, "wasi-blobstore-blobstore.ts"),
  },
]

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    globals: false,
  },
  resolve: {
    alias: golemAliases,
  },
})
