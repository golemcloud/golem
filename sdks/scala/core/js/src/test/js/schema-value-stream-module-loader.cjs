const Module = require("node:module");

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
  return load.apply(this, arguments);
};
