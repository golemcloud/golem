// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const bridge = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "tool-process-bridge",
);

function replaceOnce(text, from, to, file) {
  if (text.split(from).length !== 2) {
    throw new Error(`Expected one registration point in ${file}: ${from}`);
  }
  return text.replace(from, to);
}

export function installToolProcessBridge(wrapper) {
  const cargo = resolve(wrapper, "Cargo.toml");
  writeFileSync(
    cargo,
    replaceOnce(
      readFileSync(cargo, "utf8"),
      "[dependencies]\n",
      `[dependencies]\ngolem-js-tool-process-bridge = { path = ${JSON.stringify(bridge)} }\n`,
      cargo,
    ),
  );

  const modules = resolve(wrapper, "src/modules/mod.rs");
  let text = readFileSync(modules, "utf8");
  text = replaceOnce(
    text,
    "    resolver\n",
    '    resolver\n        .with_module("golem-tool-process-bridge")\n',
    modules,
  );
  text = replaceOnce(
    text,
    "    rquickjs::loader::ModuleLoader::default()\n",
    '    rquickjs::loader::ModuleLoader::default()\n        .with_module("golem-tool-process-bridge", golem_js_tool_process_bridge::ToolProcessBridge)\n',
    modules,
  );
  writeFileSync(modules, text);
}
