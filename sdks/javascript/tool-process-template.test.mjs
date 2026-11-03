// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import {
  templateGeneratorArgs,
  toolProcessModules,
} from "./tool-process-template.mjs";

test("template modules include the runtime initializer and portable source paths", () => {
  assert.deepEqual(
    toolProcessModules.map(([name]) => name),
    [
      "golem-tool-process-backend",
      "golem-tool-process-host",
      "wasm-rquickjs-runtime-init",
    ],
  );
  for (const [, source] of toolProcessModules) {
    assert.ok(isAbsolute(source));
    assert.ok(readFileSync(source).length > 0);
  }
});

test("generator arguments register the native bridge and the forked wit-bindgen", () => {
  const pairs = [];
  for (let index = 0; index < templateGeneratorArgs.length; index += 2) {
    pairs.push([templateGeneratorArgs[index], templateGeneratorArgs[index + 1]]);
  }
  assert.deepEqual(
    pairs.map(([flag, value]) => [flag, value.split("=")[0]]),
    [
      ["--native-module", "golem-tool-process-bridge"],
      ["--dependency", "golem-js-tool-process-bridge"],
      ["--dependency", "wit-bindgen-p3"],
    ],
  );
  assert.equal(
    pairs[0][1],
    "golem-tool-process-bridge=golem_js_tool_process_bridge::ToolProcessBridge",
  );
  const path = JSON.parse(pairs[1][1].match(/path = ("[^"]*")/)[1]);
  assert.ok(existsSync(join(path, "Cargo.toml")));
  assert.match(pairs[2][1], /git = "https:\/\/github.com\/golemcloud\/wit-bindgen"/);
});

test("every JavaScript SDK template generator embeds the modules and passes the arguments", () => {
  const sdks = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  for (const generator of [
    "ts/packages/golem-ts-sdk/scripts/generate-agent-template.mjs",
    "effect/scripts/generate-agent-template.mjs",
    "scala/scripts/generate-agent-guest-wasm.sh",
  ]) {
    const source = readFileSync(join(sdks, generator), "utf8");
    assert.match(source, /javascript\/tool-process-template\.mjs/, generator);
    assert.match(source, /\btoolProcessModules\b/, generator);
    assert.match(source, /\btemplateGeneratorArgs\b/, generator);
    assert.doesNotMatch(source, /wit-bindgen-p3 = /, generator);
  }
});
