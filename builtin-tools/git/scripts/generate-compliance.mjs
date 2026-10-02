import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const outputRoot = path.join(root, "licenses");
const check = process.argv.includes("--check");

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function normalizedNotice(file) {
  const lines = fs
    .readFileSync(file, "utf8")
    .replaceAll("\r\n", "\n")
    .split("\n");
  return `${lines.map((line) => line.trimEnd()).join("\n").trimEnd()}\n`;
}

function packageName(packagePath, manifest) {
  if (manifest.name) return manifest.name;
  return packagePath.split("node_modules/").at(-1);
}

const lock = readJson(path.join(root, "package-lock.json"));
const provenance = readJson(path.join(root, "provenance.json"));
const packages = Object.entries(lock.packages)
  .filter(
    ([packagePath, manifest]) =>
      packagePath && !manifest.dev && !manifest.link && manifest.version,
  )
  .map(([packagePath, manifest]) => ({
    name: packageName(packagePath, manifest),
    version: manifest.version,
    license: manifest.license ?? "NOASSERTION",
    source: manifest.resolved ?? "NOASSERTION",
  }))
  .sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`),
  );

const spdx = `${JSON.stringify(
  {
    spdxVersion: "SPDX-2.3",
    dataLicense: "CC0-1.0",
    SPDXID: "SPDXRef-DOCUMENT",
    name: "Golem Git built-in tool source",
    documentNamespace: `https://golem.cloud/spdx/tools/git/${provenance.isomorphicGit.artifactSha256}`,
    creationInfo: {
      created: "2026-10-02T00:00:00Z",
      creators: ["Tool: golem-builtin-git-compliance"],
    },
    packages: packages.map((item, index) => ({
      name: item.name,
      SPDXID: `SPDXRef-Package-${index + 1}`,
      versionInfo: item.version,
      downloadLocation: item.source,
      filesAnalyzed: false,
      licenseConcluded: "NOASSERTION",
      licenseDeclared: item.license,
      copyrightText: "NOASSERTION",
    })),
  },
  null,
  2,
)}\n`;

const sourceLines = ["isomorphicGit", "diff", "zod"].map((key) => {
  const item = provenance[key];
  return `- ${item.name} ${item.version}: ${item.sourceTag}\n  - Registry archive: ${item.registryTarball}\n  - Integrity: \`${item.registryIntegrity}\`\n  - Repacked artifact SHA-256: \`${item.artifactSha256}\``;
});
const generated = new Map([
  [
    "git/ISOMORPHIC-GIT-LICENSE.md",
    normalizedNotice(path.join(root, "node_modules/isomorphic-git/LICENSE.md")),
  ],
  [
    "git/DIFF-LICENSE",
    normalizedNotice(path.join(root, "node_modules/diff/LICENSE")),
  ],
  [
    "git/ZOD-LICENSE",
    normalizedNotice(path.join(root, "node_modules/zod/LICENSE")),
  ],
  [
    "git/GOLEM-TS-SDK-LICENSE",
    normalizedNotice(path.join(root, "../../sdks/ts/packages/golem-ts-sdk/LICENSE")),
  ],
  [
    "git/DEPENDENCY-NOTICES.md",
    `# Git built-in tool dependency notices\n\nGenerated from the pinned production package tree.\n\n| Package | Version | License | Source |\n| --- | --- | --- | --- |\n${packages
      .map(
        (item) =>
          `| ${item.name.replaceAll("|", "\\|")} | ${item.version} | ${item.license} | ${item.source} |`,
      )
      .join("\n")}\n`,
  ],
  ["git/sbom.spdx.json", spdx],
  [
    "SOURCE.md",
    `# Git tool source and notices\n\nThe Git object and worktree implementation is provided by pinned upstream packages. Golem-owned wrappers expose the supported local command surface and integrate it with the component filesystem.\n\n${sourceLines.join("\n")}\n\nThe Git and upstream package names are used descriptively. No upstream logos are used, no endorsement is implied, and the respective trademarks belong to their owners. Production publication requires legal and release-owner approval.\n`,
  ],
]);

let stale = false;
for (const [relative, content] of generated) {
  const target = path.join(outputRoot, relative);
  if (check) {
    if (!fs.existsSync(target) || fs.readFileSync(target, "utf8") !== content) {
      console.error(`stale compliance artifact: ${path.relative(root, target)}`);
      stale = true;
    }
  } else {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
}

if (check && stale) process.exitCode = 1;
else {
  const digest = crypto
    .createHash("sha256")
    .update([...generated.values()].join("\0"))
    .digest("hex");
  console.log(`${check ? "verified" : "generated"} compliance bundle ${digest}`);
}
