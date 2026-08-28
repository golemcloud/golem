const Module = require("node:module");

const load = Module._load;
Module._load = function (request) {
  if (request === "golem:core/types@2.0.0") {
    return {
      SchemaValueStream: {
        wrap: async (iterable) => ({ iterable }),
        unwrap: async (stream) => stream.iterable,
      },
    };
  }
  return load.apply(this, arguments);
};
