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
import { rollup } from "../../../../sdks/ts/packages/golem-ts-sdk/node_modules/rollup/dist/es/rollup.js";
import { nodeResolve } from "../../../../sdks/ts/packages/golem-ts-sdk/node_modules/@rollup/plugin-node-resolve/dist/es/index.js";
import commonjs from "../../../../sdks/ts/packages/golem-ts-sdk/node_modules/@rollup/plugin-commonjs/dist/es/index.js";
import {
  installToolProcessBridge,
  toolProcessModules,
} from "../../../../sdks/javascript/tool-process-template.mjs";
import { componentEntry } from "../../../../sdks/ts/packages/golem-ts-sdk/scripts/component.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../../..");
const destination = process.argv[2];
if (!destination) throw new Error("Pass an empty output directory");
const sdkMinimal = process.argv[3] === "--sdk-minimal";
const sdkOnly = sdkMinimal || process.argv[3] === "--sdk-only";
const sdkTemplate = process.argv[3] === "--sdk-template";
if (process.argv[3] && !sdkOnly && !sdkTemplate)
  throw new Error("Unknown probe build option");
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
    input: sdkTemplate ? "virtual:probe" : join(here, input),
    external: (id) =>
      /^(node:|wasi:|golem:|wasm-rquickjs:|golem-tool-process-)/.test(id) &&
      !(sdkTemplate && id === "golem-tool-process-sdk-control"),
    treeshake: { moduleSideEffects: (id) => !/^(wasi:|golem:)/.test(id) },
    plugins: [
      {
        name: "sdk-template-probe",
        resolveId(id) {
          if (id === "virtual:probe") return id;
          if (sdkTemplate && id === "golem-tool-process-sdk-control")
            return join(here, "sdk-control.js");
        },
        load(id) {
          if (id === "virtual:probe")
            return componentEntry(
              join(here, input),
              { agents: false, tools: true, middleware: false },
              undefined,
            );
        },
      },
      nodeResolve(),
      commonjs(),
    ],
    onwarn(warning, warn) {
      if (warning.code !== "CIRCULAR_DEPENDENCY") warn(warning);
    },
  });
  await bundle.write({ file, format: "es", inlineDynamicImports: true });
  await bundle.close();
}
await bundleModule(sdkOnly ? "sdk-only-probe.js" : "tool-probe.js", source);
if (sdkTemplate) {
  const injected = join(out, "injected.wasm");
  run("wasm-rquickjs", [
    "inject-js",
    "--input",
    join(root, "sdks/ts/packages/golem-ts-sdk/wasm/agent_guest.wasm"),
    "--output",
    injected,
    "--js",
    source,
  ]);
  run("wasm-rquickjs", [
    "optimize",
    "--input",
    injected,
    "--output",
    join(out, "probe.wasm"),
  ]);
  process.exit(0);
}
const sdkControl = join(out, "sdk-control.mjs");
await bundleModule(
  sdkMinimal ? "sdk-replay-minimal.js" : "sdk-control.js",
  sdkControl,
);

const modules = [
  `user=${source}`,
  `golem-tool-process-sdk-control=${sdkControl}`,
];
if (!sdkOnly)
  modules.push(
    ...toolProcessModules.map(([name, source]) => `${name}=${source}`),
  );
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
if (!sdkOnly) installToolProcessBridge(wrapper);

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
