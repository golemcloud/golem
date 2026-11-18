// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import {
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// Each SDK supplies its complete generated file set, so deleted fixtures cannot linger.
export function checkBundleFixtures(sdk, files) {
  const directory = fileURLToPath(
    new URL(
      `../../test-components/js-bundle-fixtures/${sdk}/`,
      import.meta.url,
    ),
  );
  const update = process.env.GOLEM_UPDATE_JS_BUNDLES === "1";
  if (update) {
    mkdirSync(directory, { recursive: true });
    for (const name of readdirSync(directory)) {
      if (!Object.hasOwn(files, name)) rmSync(join(directory, name));
    }
    for (const [name, source] of Object.entries(files)) {
      writeFileSync(join(directory, name), source);
    }
    return;
  }
  const expected = Object.keys(files).sort();
  let actual;
  try {
    actual = readdirSync(directory).sort();
  } catch (cause) {
    throw new Error(
      `Missing ${sdk} JS bundle fixtures; run cargo make generate-js-bundle-fixtures`,
      {
        cause,
      },
    );
  }
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Stale ${sdk} JS bundle fixture file set; run cargo make generate-js-bundle-fixtures`,
    );
  }
  for (const [name, source] of Object.entries(files)) {
    if (!readFileSync(join(directory, name)).equals(Buffer.from(source))) {
      throw new Error(
        `Stale ${sdk}/${name}; run cargo make generate-js-bundle-fixtures`,
      );
    }
  }
}
