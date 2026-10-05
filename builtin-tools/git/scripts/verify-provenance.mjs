import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = path.resolve(import.meta.dirname, "..");
const provenance = JSON.parse(
  fs.readFileSync(path.join(root, "provenance.json"), "utf8"),
);
const lock = JSON.parse(
  fs.readFileSync(path.join(root, "package-lock.json"), "utf8"),
);
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "golem-git-verify-"));

try {
  for (const key of ["isomorphicGit", "diff", "zod"]) {
    const expected = provenance[key];
    const packageRoot = path.join(root, "node_modules", expected.name);
    const manifest = JSON.parse(
      fs.readFileSync(path.join(packageRoot, "package.json"), "utf8"),
    );
    const locked = lock.packages[`node_modules/${expected.name}`];
    if (manifest.version !== expected.version || locked?.version !== expected.version) {
      throw new Error(`${expected.name} version drifted from ${expected.version}`);
    }
    if (locked?.resolved !== expected.registryTarball) {
      throw new Error(`${expected.name} registry tarball drifted`);
    }
    if (locked?.integrity !== expected.registryIntegrity) {
      throw new Error(`${expected.name} registry integrity drifted`);
    }
    const packed = spawnSync(
      "npm",
      [
        "pack",
        packageRoot,
        "--ignore-scripts",
        "--silent",
        "--cache",
        path.join(temporary, "cache"),
        "--pack-destination",
        temporary,
      ],
      { cwd: root, encoding: "utf8" },
    );
    if (packed.status !== 0) {
      throw new Error(packed.stderr || `npm pack failed for ${expected.name}`);
    }
    const archive = packed.stdout.trim().split(/\r?\n/).at(-1);
    const digest = crypto
      .createHash("sha256")
      .update(fs.readFileSync(path.join(temporary, archive)))
      .digest("hex");
    if (digest !== expected.artifactSha256) {
      throw new Error(
        `${expected.name} artifact SHA-256 drifted: expected ${expected.artifactSha256}, got ${digest}`,
      );
    }
    console.log(`verified ${expected.name}@${expected.version} ${digest}`);
  }
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
