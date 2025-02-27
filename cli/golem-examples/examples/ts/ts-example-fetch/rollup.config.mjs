import typescript from "rollup-plugin-typescript2";
import resolve from "@rollup/plugin-node-resolve";

export default {
  input: "src/main.ts",
  output: {
    file: "out/main.js",
    format: "esm",
  },
  external: ["golem:api/host@1.1.0"],
  plugins: [resolve(), typescript()],
};
