// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { rmSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const directory = dirname(fileURLToPath(import.meta.url));
const bridge = resolve(directory, "tool-process-bridge");

export const toolProcessModules = [
  ["golem-tool-process-backend", resolve(directory, "tool-process-backend.js")],
  ["golem-tool-process-host", resolve(directory, "tool-process-host.js")],
  [
    "wasm-rquickjs-runtime-init",
    resolve(directory, "tool-process-runtime-init.js"),
  ],
];

// Golem's wit-bindgen fork adds an "outline-lift" optimization that shrinks the generated
// Preview 3 lift/lower wrappers. It replaces the skeleton's `wit-bindgen-p3` entry as a whole.
const witBindgenP3 =
  '{ package = "wit-bindgen", git = "https://github.com/golemcloud/wit-bindgen", ' +
  'rev = "4407232ead86d9bcbd06cbebd790a52120a4087a", version = "=0.59.0", ' +
  'default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], ' +
  "optional = true }";

// `wasm-rquickjs generate-wrapper-crate` arguments shared by the SDK agent templates: the
// native `golem-tool-process-bridge` module from the Golem-owned bridge crate, and the forked
// wit-bindgen. The JavaScript modules are passed separately with `--js-modules`.
export const templateGeneratorArgs = [
  "--native-module",
  "golem-tool-process-bridge=golem_js_tool_process_bridge::ToolProcessBridge",
  "--dependency",
  `golem-js-tool-process-bridge={ path = ${JSON.stringify(bridge)} }`,
  "--dependency",
  `wit-bindgen-p3=${witBindgenP3}`,
];

// The generated Cargo.lock pins the skeleton's upstream wit-bindgen dependencies, which
// conflict with the fork (its wit-parser needs a newer indexmap). Cargo resolves a fresh lock
// during the template build instead.
export function removeSkeletonLock(wrapper) {
  rmSync(resolve(wrapper, "Cargo.lock"), { force: true });
}
