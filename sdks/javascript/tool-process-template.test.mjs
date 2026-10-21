// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, isAbsolute } from "node:path";
import { test } from "node:test";
import {
  installToolProcessBridge,
  toolProcessModules,
} from "./tool-process-template.mjs";

function wrapper(t, supported = true) {
  const directory = mkdtempSync(join(tmpdir(), "golem-process-template-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(join(directory, "src/builtin"), { recursive: true });
  mkdirSync(join(directory, "src/modules"));
  writeFileSync(join(directory, "Cargo.toml"), "[dependencies]\n");
  writeFileSync(
    join(directory, "src/builtin/mod.rs"),
    supported
      ? 'resolver.with_module("wasm-rquickjs:child-process")'
      : "resolver",
  );
  writeFileSync(
    join(directory, "src/modules/mod.rs"),
    "    resolver\n    rquickjs::loader::ModuleLoader::default()\n",
  );
  return directory;
}

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

test("the template installs the Golem native module and dependency", (t) => {
  const directory = wrapper(t);
  installToolProcessBridge(directory);
  assert.match(
    readFileSync(join(directory, "Cargo.toml"), "utf8"),
    /golem-js-tool-process-bridge = \{ path = /,
  );
  assert.match(
    readFileSync(join(directory, "src/modules/mod.rs"), "utf8"),
    /golem_js_tool_process_bridge::ToolProcessBridge/,
  );
  assert.match(
    readFileSync(join(directory, "src/modules/mod.rs"), "utf8"),
    /\.with_module\("golem-tool-process-bridge"\)/,
  );
});

test("an unsupported runtime fails before changing the generated crate", (t) => {
  const directory = wrapper(t, false);
  assert.throws(
    () => installToolProcessBridge(directory),
    /must provide runtime-local child process backends/,
  );
  assert.equal(
    readFileSync(join(directory, "Cargo.toml"), "utf8"),
    "[dependencies]\n",
  );
});

test("changed native module registration points fail closed", (t) => {
  const directory = wrapper(t);
  writeFileSync(join(directory, "src/modules/mod.rs"), "changed skeleton");
  assert.throws(
    () => installToolProcessBridge(directory),
    /Expected one registration point/,
  );
});
