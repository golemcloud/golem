const Module = require("node:module");

function missingHostImport(name) {
  return (...args) => {
    const implementation = globalThis.__golemScalaTestHost?.[name];
    if (typeof implementation === "function") {
      return implementation(...args);
    }
    throw new Error(`Unexpected Golem host import in Scala.js tests: ${name}`);
  };
}

const durableStreamResources = { readers: [], writers: [] };
globalThis.__golemDurableStreamResources = durableStreamResources;

function durableStreamResource(kind, method, requestKeys) {
  return class {
    constructor(options, auth) {
      this.options = Object.freeze({ ...options });
      this.auth = auth;
      this.requests = [];
      this.active = 0;
      this.drops = 0;
      durableStreamResources[kind].push(this);
      globalThis.__golemScalaTestHost?.constructStream?.(this);
    }
    async [method](request) {
      if (this.drops) throw new Error("using a dropped Durable Streams resource");
      if (Object.keys(request).sort().join() !== requestKeys.join()) {
        throw new Error(`unexpected ${method} request fields: ${Object.keys(request)}`);
      }
      this.requests.push(request);
      this.active++;
      try {
        return await missingHostImport(`${method}Stream`)(request, this);
      } finally {
        this.active--;
      }
    }
    [Symbol.dispose]() {
      if (this.active) throw new Error("dropping a borrowed Durable Streams resource");
      if (this.drops++) throw new Error("dropping a Durable Streams resource twice");
    }
  };
}

const load = Module._load;
const state = {
  wraps: 0,
  unwraps: 0,
  failWrapAt: 0,
  deferWrapAt: 0,
  pendingWraps: [],
};

globalThis.__golemSchemaValueStreamMock = {
  state,
  reset() {
    state.wraps = 0;
    state.unwraps = 0;
    state.failWrapAt = 0;
    state.deferWrapAt = 0;
    state.pendingWraps.length = 0;
  },
  async waitForPendingWrap() {
    while (state.pendingWraps.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  },
  releaseWraps() {
    const pending = state.pendingWraps.splice(0);
    for (const release of pending) release();
  },
};

Module._load = function (request) {
  if (request === "golem:core/types@2.0.0") {
    return {
      SchemaValueStream: {
        wrap: async (iterable) => {
          state.wraps += 1;
          if (state.failWrapAt === state.wraps) {
            throw new Error(`schema stream wrap ${state.wraps} failed`);
          }
          if (state.deferWrapAt === state.wraps) {
            await new Promise((resolve) => {
              state.pendingWraps.push(resolve);
            });
          }
          return { iterable };
        },
        unwrap: async (stream) => {
          state.unwraps += 1;
          return stream.iterable;
        },
      },
    };
  }
  if (request === "golem:agent/host@2.0.0") {
    return {
      getConfigValue: missingHostImport("getConfigValue"),
      parseAgentId: missingHostImport("parseAgentId"),
    };
  }
  if (request === "golem:tool/host@0.1.0") {
    return {
      createStdin: missingHostImport("createStdin"),
      createStdout: missingHostImport("createStdout"),
      ToolRpc: missingHostImport("ToolRpc"),
    };
  }
  if (request === "golem:agent/durable-streams@2.0.0") {
    return {
      DurableStreamReader: durableStreamResource("readers", "read", ["checkpoint", "contentType", "transport"]),
      DurableStreamWriter: durableStreamResource("writers", "append", ["close", "payload", "sequence"]),
    };
  }
  if (request === "golem:api/host@1.5.0") {
    return {
      getSelfMetadata: missingHostImport("getSelfMetadata"),
      generateIdempotencyKey: missingHostImport("generateIdempotencyKey"),
    };
  }
  if (request === "wasi:clocks/monotonic-clock@0.3.0") {
    return {
      now: missingHostImport("now"),
      waitFor: missingHostImport("waitFor"),
    };
  }
  if (request === "golem:secrets/reveal@0.1.0") {
    return { reveal: missingHostImport("reveal") };
  }
  return load.apply(this, arguments);
};
