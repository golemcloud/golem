import assert from "node:assert/strict";
import { register } from "node:module";
import { loadFixture } from "./check.mjs";

const [providerPath, clientPath] = process.argv.slice(2);
const { linked: provider } = await loadFixture(providerPath);
let invocations = 0;
globalThis.__capabilityToolInvoke = (name, path, input, stdin, stdout, stderr) => {
  invocations++;
  assert.equal(name, "echo");
  assert.deepEqual(path, []);
  assert.equal(stdin, undefined);
  assert.equal(stdout, undefined);
  assert.equal(stderr, undefined);
  return provider.golemTool010Guest.invoke(name, path, input, stdin, stdout, stderr, {
    tag: "anonymous",
  });
};
register("./host-module-hooks.mjs", import.meta.url, {
  data: {
    modules: [
      [
        "golem:tool/host@0.1.0",
        `export class ToolRpc {
        constructor(name) { this.name = name; }
        static create(name) { return new ToolRpc(name); }
        asyncInvokeAndAwait(...args) {
          const result = globalThis.__capabilityToolInvoke(this.name, ...args);
          return { get: () => result, cancel() { throw new Error("unexpected cancellation"); } };
        }
      }`,
      ],
    ],
  },
});
const ambient = await import("golem:agent/host@2.0.0");
assert.throws(
  () => ambient.parseAgentId(),
  /Unexpected host call: golem:agent\/host@2\.0\.0/,
);
const { linked: client, source } = await loadFixture(clientPath);
assert.deepEqual(client.golemAgent200Guest.discoverAgentTypes(), []);
assert.deepEqual(client.golemTool010Guest.discoverTools(), []);
assert.deepEqual(
  client.golemTool010ToolMiddlewareGuest.discoverToolMiddlewares(),
  [],
);
assert(source.includes('from "golem:tool/host@0.1.0"'));
assert(!source.includes('from "golem:agent/host@'));
assert.equal(await client.callEcho("client→provider"), "echo:client→provider");
assert.equal(invocations, 1);
delete globalThis.__capabilityToolInvoke;
console.log(
  "client-only: generated RPC roundtrip through a host stub verified; local discovery stays empty",
);
