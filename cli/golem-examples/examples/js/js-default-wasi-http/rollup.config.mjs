import resolve from "@rollup/plugin-node-resolve";

export default {
  input: "src/main.js",
  output: {
    file: "out/main.js",
    format: "esm",
  },
  external: ["wasi:http/types@0.2.0"],
  plugins: [resolve()],
};
