import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { pathToFileURL } from "node:url";

// These fixtures do not use streams or ambient host APIs. Resolve the static
// imports, but fail if a fixture unexpectedly calls one.
const hostModules = new Set([
  "golem:core/types@2.0.0",
  "golem:agent/host@2.0.0",
  "golem:api/host@1.5.0",
]);
registerHooks({
  resolve(specifier, context, next) {
    if (hostModules.has(specifier)) {
      const source = `
        const unavailable = () => { throw new Error("Unexpected host call: ${specifier}"); };
        export const SchemaValueStream = { wrap: unavailable, unwrap: unavailable };
        export const parseAgentId = unavailable;
        export const getSelfMetadata = unavailable;
      `;
      return {
        url: `data:text/javascript,${encodeURIComponent(source)}`,
        shortCircuit: true,
      };
    }
    return next(specifier, context);
  },
});

export async function loadFixture(path) {
  const source = readFileSync(path, "utf8");
  const linked = await import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
  );
  return { source, linked };
}

const metadata = { aliases: [], examples: [] };
const stringSchema = {
  typeNodes: [{ body: { tag: "string-type" }, metadata }],
  defs: [],
  root: 0,
};
const record = (tag, value) => ({
  valueNodes: [
    { tag, val: value },
    { tag: "record-value", val: [0] },
  ],
  root: 1,
});
export const toolInput = (value) => ({
  graph: {
    typeNodes: [
      {
        body: {
          tag: "record-type",
          val: [{ name: "value", body: 1, metadata }],
        },
        metadata,
      },
      { body: { tag: "string-type" }, metadata },
    ],
    defs: [],
    root: 0,
  },
  value: record("string-value", value),
});
const anonymous = { tag: "anonymous" };
const errorTag = (tag) => (error) => error?.tag === tag;

export async function checkFixture(path, capabilities) {
  const { source, linked } = await loadFixture(path);
  const agents = capabilities === "agent" || capabilities === "mixed";
  const tools = capabilities === "tool" || capabilities === "mixed";
  const middleware = capabilities === "middleware" || capabilities === "mixed";
  const agent = linked.golemAgent200Guest;
  const tool = linked.golemTool010Guest;
  const mw = linked.golemTool010ToolMiddlewareGuest;
  for (const [object, names] of [
    [agent, ["initialize", "invoke", "getDefinition", "discoverAgentTypes"]],
    [tool, ["discoverTools", "getTool", "invoke"]],
    [
      mw,
      ["discoverToolMiddlewares", "getToolMiddleware", "invokeToolMiddleware"],
    ],
    [linked.saveSnapshot, ["save"]],
    [linked.loadSnapshot, ["load"]],
  ]) {
    for (const name of names)
      assert.equal(typeof object?.[name], "function", name);
  }
  assert.equal(linked.guest, agent);
  assert.equal(linked.toolMiddlewareGuest, mw);
  // Discovery must be synchronous, including empty capabilities.
  assert.deepEqual(
    agent.discoverAgentTypes().map((a) => a.typeName),
    agents ? ["counter"] : [],
  );
  assert.deepEqual(
    tool.discoverTools().map((t) => t.commands.nodes[0].name),
    tools ? ["echo"] : [],
  );
  assert.deepEqual(
    mw.discoverToolMiddlewares().map((m) => m.name),
    middleware ? ["pass-through"] : [],
  );
  assert.throws(() => tool.getTool("missing"), errorTag("invalid-tool-name"));
  assert.throws(
    () => mw.getToolMiddleware("missing"),
    errorTag("invalid-tool-name"),
  );
  await assert.rejects(
    mw.invokeToolMiddleware("missing"),
    errorTag("invalid-tool-name"),
  );

  const snapshot = await linked.saveSnapshot.save();
  assert.equal(snapshot.mimeType, "application/octet-stream");
  assert.deepEqual([...snapshot.payload], []);
  if (!agents) {
    assert(!source.includes('from "golem:agent/host@'));
    assert(!source.includes('from "golem:api/host@'));
    await assert.rejects(
      agent.initialize("counter", record("string-value", "three"), anonymous),
      errorTag("invalid-type"),
    );
    await assert.rejects(
      agent.invoke("add", record("s32-value", 7), anonymous),
      errorTag("invalid-agent-id"),
    );
    assert.throws(() => agent.getDefinition(), errorTag("invalid-agent-id"));
    await assert.rejects(
      linked.loadSnapshot.load(snapshot),
      (e) => e === "No agents registered",
    );
  } else {
    await agent.initialize(
      "counter",
      record("string-value", "three"),
      anonymous,
    );
    assert.equal(agent.getDefinition().typeName, "counter");
    const first = await agent.invoke("add", record("s32-value", 7), anonymous);
    assert.equal(first.valueNodes[first.root].val, 12);
    const second = await agent.invoke(
      "add",
      record("s32-value", -2),
      anonymous,
    );
    assert.equal(second.valueNodes[second.root].val, 10);
    await assert.rejects(
      agent.initialize("counter", record("string-value", "again"), anonymous),
      errorTag("custom-error"),
    );
    await assert.rejects(
      linked.loadSnapshot.load(snapshot),
      errorTag("custom-error"),
    );
  }
  if (tools) {
    assert.equal(tool.getTool("echo").commands.nodes[0].name, "echo");
    const result = await tool.invoke(
      "echo",
      [],
      toolInput("héllo"),
      undefined,
      undefined,
      anonymous,
    );
    assert.equal(
      result.result.value.valueNodes[result.result.value.root].val,
      "echo:héllo",
    );
    await assert.rejects(
      tool.invoke(
        "echo",
        ["missing"],
        toolInput("x"),
        undefined,
        undefined,
        anonymous,
      ),
      errorTag("invalid-command-path"),
    );
  } else {
    await assert.rejects(
      tool.invoke("echo", [], toolInput("x"), undefined, undefined, anonymous),
      errorTag("invalid-tool-name"),
    );
  }
  if (middleware) {
    assert.equal(mw.getToolMiddleware("pass-through").scope.tag, "universal");
    let invoked = 0;
    const underlying = {
      async invoke(path, input, stdin) {
        invoked++;
        assert.deepEqual(path, ["nested"]);
        assert.equal(input.value.valueNodes[input.value.root].val, "request");
        assert.equal(stdin, undefined);
        return [
          {
            get: async () => ({
              graph: stringSchema,
              value: {
                valueNodes: [{ tag: "string-value", val: "response" }],
                root: 0,
              },
            }),
            cancel() {},
            [Symbol.dispose]() {},
          },
          undefined,
        ];
      },
    };
    const descriptor = mw.getToolMiddleware("pass-through");
    const result = await mw.invokeToolMiddleware(
      "pass-through",
      "underlying",
      { version: "1.0.0", commands: { nodes: [] }, schema: stringSchema },
      {
        graph: descriptor.parameterSchema,
        value: { valueNodes: [{ tag: "record-value", val: [] }], root: 0 },
      },
      ["nested"],
      {
        graph: stringSchema,
        value: {
          valueNodes: [{ tag: "string-value", val: "request" }],
          root: 0,
        },
      },
      undefined,
      undefined,
      anonymous,
      underlying,
    );
    assert.equal(invoked, 1);
    assert.equal(
      result.result.value.valueNodes[result.result.value.root].val,
      "response",
    );
  }
  if (capabilities === "empty") assert(!source.includes('from "golem:'));
  assert(
    !source.includes('from "golem:tool/host@'),
    "guest dispatch must not import ambient tool RPC",
  );
  console.log(
    `${capabilities}: exports, discovery, invocation, snapshots, import pruning verified (${Buffer.byteLength(source)} bytes)`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [capabilities, path] = process.argv.slice(2);
  assert(
    ["empty", "tool", "agent", "middleware", "mixed"].includes(capabilities),
    "usage: node check.mjs <empty|tool|agent|middleware|mixed> <linked-main.js>",
  );
  await checkFixture(path, capabilities);
}
