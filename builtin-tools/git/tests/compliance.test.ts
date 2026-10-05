import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { collectLicenseAssets } from "../scripts/compliance-policy.mjs";

function packageDirectory(files: Record<string, string>): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "git-compliance-"));
  for (const [relative, content] of Object.entries(files)) {
    const target = path.join(root, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
  return root;
}

test("compliance rejects an unknown license even when a NOTICE exists", () => {
  const root = packageDirectory({ NOTICE: "some notice\n" });
  try {
    assert.throws(
      () =>
        collectLicenseAssets(
          { name: "unknown", version: "1.0.0", license: "NOASSERTION", root },
          () => [],
        ),
      /unreviewed license declaration/,
    );
  } finally {
    fs.rmSync(root, { recursive: true });
  }
});

test("compliance requires pako's reviewed supplemental Zlib notice", () => {
  const root = packageDirectory({ LICENSE: "MIT\n", "lib/zlib/deflate.js": "no notice\n" });
  try {
    assert.throws(
      () =>
        collectLicenseAssets(
          {
            name: "pako",
            version: "1.0.11",
            license: "(MIT AND Zlib)",
            root,
          },
          () => [],
        ),
      /missing reviewed Zlib source notice/,
    );
  } finally {
    fs.rmSync(root, { recursive: true });
  }
});
