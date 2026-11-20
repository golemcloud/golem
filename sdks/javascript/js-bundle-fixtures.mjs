// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Ordinary SDK tests bundle in memory. The cargo-make preparation task saves
// those same outputs for the process-free Rust tests.
export function writeBundleFixture(sdk, fixture, files) {
  const root = process.env.GOLEM_JS_GUEST_BUNDLE_DIR;
  if (!root) return;
  const directory = join(root, sdk, fixture);
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
  for (const [name, source] of Object.entries(files)) {
    writeFileSync(join(directory, name), source);
  }
}

export function bundleSourcePath(source) {
  const normalized = source.replaceAll("\\", "/");
  const dependency = normalized.lastIndexOf("/node_modules/");
  return dependency < 0 ? normalized : normalized.slice(dependency + 1);
}
