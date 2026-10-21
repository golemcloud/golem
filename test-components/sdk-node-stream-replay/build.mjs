// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { rollup } from "../../sdks/ts/packages/golem-ts-sdk/node_modules/rollup/dist/es/rollup.js";
import { nodeResolve } from "../../sdks/ts/packages/golem-ts-sdk/node_modules/@rollup/plugin-node-resolve/dist/es/index.js";
import commonjs from "../../sdks/ts/packages/golem-ts-sdk/node_modules/@rollup/plugin-commonjs/dist/es/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const destination = process.argv[2];
if (!destination) throw new Error("Pass an empty output directory");
if (process.argv[3]) throw new Error("Unknown probe build option");
const out = resolve(destination);
const wrapper = join(out, "wrapper");
const source = join(out, "probe.mjs");
if (
  existsSync(wrapper) ||
  existsSync(source) ||
  existsSync(join(out, "probe.wasm"))
)
  throw new Error("Output directory already contains a probe build");
mkdirSync(out, { recursive: true });

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

async function bundleModule(input, file) {
  const bundle = await rollup({
    input: join(here, input),
    external: (id) =>
      /^(node:|wasi:|golem:|wasm-rquickjs:|golem-tool-process-)/.test(id),
    treeshake: { moduleSideEffects: (id) => !/^(wasi:|golem:)/.test(id) },
    plugins: [nodeResolve(), commonjs()],
    onwarn(warning, warn) {
      if (warning.code !== "CIRCULAR_DEPENDENCY") warn(warning);
    },
  });
  await bundle.write({ file, format: "es" });
  await bundle.close();
}
await bundleModule("probe.js", source);
const sdkControl = join(out, "sdk-control.mjs");
await bundleModule("control.js", sdkControl);

const modules = [
  `user=${source}`,
  `golem-tool-process-sdk-control=${sdkControl}`,
];
run("wasm-rquickjs", [
  "generate-wrapper-crate",
  "--wit",
  join(root, "sdks/ts/wit"),
  "--output",
  wrapper,
  "--world",
  "agent-guest",
  "--target",
  "wasi-p3",
  ...modules.flatMap((module) => ["--js-modules", module]),
]);

// Match the SDK generator's binding revision rather than the WRQJS default.
const generator = readFileSync(
  join(
    root,
    "sdks/ts/packages/golem-ts-sdk/scripts/generate-agent-template.mjs",
  ),
  "utf8",
);
const revision = generator.match(
  /const WIT_BINDGEN_REVISION = '([a-f0-9]+)'/,
)[1];
const cargo = join(wrapper, "Cargo.toml");
const original = readFileSync(cargo, "utf8");
const pinned = original.replace(
  'package = "wit-bindgen", version = "0.58.0"',
  `package = "wit-bindgen", git = "https://github.com/golemcloud/wit-bindgen", rev = "${revision}", version = "=0.59.0"`,
);
if (pinned === original)
  throw new Error("Cannot select the SDK binding revision");
const rewritten = pinned.replace(
  'name = "agent-guest"',
  'name = "golem-node-tool-process-probe"',
);
if (rewritten === pinned) throw new Error("Cannot name the probe package");
writeFileSync(cargo, rewritten);
rmSync(join(wrapper, "Cargo.lock"));

run("cargo", [
  "build",
  "--offline",
  "--manifest-path",
  cargo,
  "--target",
  "wasm32-wasip2",
  "--release",
  "--target-dir",
  join(root, "sdks/ts/packages/golem-ts-sdk/agent-template/target"),
]);
const wasm = join(
  root,
  "sdks/ts/packages/golem-ts-sdk/agent-template/target/wasm32-wasip2/release/golem_node_tool_process_probe.wasm",
);
run("wasm-rquickjs", [
  "optimize",
  "--input",
  wasm,
  "--output",
  join(out, "probe.wasm"),
]);
