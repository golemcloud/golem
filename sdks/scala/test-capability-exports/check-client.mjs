import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { loadFixture } from "./check.mjs";

const [providerPath, clientPath] = process.argv.slice(2);
const { linked: provider } = await loadFixture(providerPath);
let invocations = 0;
globalThis.__capabilityToolInvoke = (name, path, input, stdin, stdout) => {
  invocations++;
  assert.equal(name, "echo");
  assert.deepEqual(path, []);
  assert.equal(stdin, undefined);
  assert.equal(stdout, undefined);
  return provider.golemTool010Guest.invoke(name, path, input, stdin, stdout, {
    tag: "anonymous",
  });
};
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "golem:tool/host@0.1.0") {
      const source = `export class ToolRpc {
        constructor(name) { this.name = name; }
        asyncInvokeAndAwait(...args) {
          const result = globalThis.__capabilityToolInvoke(this.name, ...args);
          return { get: () => result, cancel() { throw new Error("unexpected cancellation"); } };
        }
      }`;
      return {
        url: `data:text/javascript,${encodeURIComponent(source)}`,
        shortCircuit: true,
      };
    }
    return next(specifier, context);
  },
});
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
