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

function packageDirectoryName(item) {
  return `${item.name.replaceAll("@", "").replaceAll("/", "__")}@${item.version}`;
}

function stripLineCommentNotice(file, lineCount) {
  return `${fs
    .readFileSync(file, "utf8")
    .replaceAll("\r\n", "\n")
    .split("\n")
    .slice(0, lineCount)
    .map((line) => line.replace(/^\/\/ ?/, ""))
    .join("\n")
    .trimEnd()}\n`;
}

function stripBlockCommentNotice(file) {
  const source = fs.readFileSync(file, "utf8").replaceAll("\r\n", "\n");
  const end = source.indexOf("*/");
  if (!source.startsWith("/*") || end === -1) {
    throw new Error(`missing leading license notice in ${file}`);
  }
  return `${source
    .slice(2, end)
    .split("\n")
    .map((line) => line.replace(/^\s*\* ?/, ""))
    .join("\n")
    .trim()}\n`;
}

function canonicalApacheLicense() {
  const source = normalizedNotice(
    path.join(root, "node_modules/crc-32/LICENSE"),
  );
  const packageNotice = source.indexOf("\n   Copyright (C) 2014-present");
  if (packageNotice === -1) {
    throw new Error("could not isolate the canonical Apache-2.0 license text");
  }
  return `${source.slice(0, packageNotice).trimEnd()}\n`;
}

function packageMetadata(item, explanation) {
  return `Package: ${item.name}@${item.version}\nDeclared license: ${item.license}\nUpstream author metadata: ${item.author ?? "not provided"}\n\n${explanation}\n\n`;
}

function fallbackLicenseAssets(item) {
  if (item.name === "clean-git-ref") {
    return [
      {
        name: "PACKAGE-LICENSE.txt",
        content:
          packageMetadata(
            item,
            "The published package declares Apache-2.0 but contains no license file. The canonical Apache License 2.0 text follows.",
          ) + canonicalApacheLicense(),
      },
    ];
  }
  if (item.name === "diff3") {
    return [
      {
        name: "diff3.js.LICENSE",
        content: stripLineCommentNotice(
          path.join(item.root, "diff3.js"),
          22,
        ),
      },
      {
        name: "onp.js.LICENSE",
        content: stripBlockCommentNotice(path.join(item.root, "onp.js")),
      },
    ];
  }
  if (item.name === "minimisted") {
    const mitTerms = stripLineCommentNotice(
      path.join(root, "node_modules/diff3/diff3.js"),
      22,
    )
      .split("\n")
      .slice(3)
      .join("\n");
    return [
      {
        name: "PACKAGE-LICENSE.txt",
        content:
          packageMetadata(
            item,
            "The published package declares MIT and names the author below, but contains no standalone license text. The canonical MIT grant and conditions follow without an invented copyright notice.",
          ) + mitTerms,
      },
    ];
  }
  throw new Error(
    `${item.name}@${item.version} has no license or notice file and no reviewed fallback`,
  );
}

const lock = readJson(path.join(root, "package-lock.json"));
const provenance = readJson(path.join(root, "provenance.json"));
const packages = Object.entries(lock.packages)
  .filter(
    ([packagePath, manifest]) =>
      packagePath && !manifest.dev && !manifest.link && manifest.version,
  )
  .map(([packagePath, manifest]) => {
    const packageRoot = path.resolve(root, packagePath);
    const installedManifest = readJson(path.join(packageRoot, "package.json"));
    return {
      name: packageName(packagePath, installedManifest),
      version: manifest.version,
      license: manifest.license ?? installedManifest.license ?? "NOASSERTION",
      source: manifest.resolved ?? "NOASSERTION",
      author:
        typeof installedManifest.author === "string"
          ? installedManifest.author
          : installedManifest.author?.name,
      root: packageRoot,
      licenseAssets: [],
    };
  })
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

for (const item of packages) {
  const licenseFiles = fs
    .readdirSync(item.root, { withFileTypes: true })
    .filter(
      (entry) =>
        entry.isFile() &&
        /^(licen[cs]e|copying|notice)(?:[-._]|$)/i.test(entry.name),
    )
    .map((entry) => ({
      name: entry.name,
      content: normalizedNotice(path.join(item.root, entry.name)),
    }));
  const assets = licenseFiles.length ? licenseFiles : fallbackLicenseAssets(item);
  const directory = `git/dependencies/${packageDirectoryName(item)}`;
  for (const asset of assets) {
    const relative = `${directory}/${asset.name}`;
    item.licenseAssets.push(relative.replace(/^git\//, ""));
    generated.set(relative, asset.content);
  }
}

generated.set(
  "git/DEPENDENCY-NOTICES.md",
  `# Git built-in tool dependency notices\n\nGenerated from the pinned production package tree. Every production package has its applicable license or source notice in the listed bundle path. Packages whose registry archives omit a standalone license use an explicitly reviewed fallback that retains the upstream license declaration and author or source-header notices.\n\n| Package | Version | License | Source | License assets |\n| --- | --- | --- | --- | --- |\n${packages
    .map(
      (item) =>
        `| ${item.name.replaceAll("|", "\\|")} | ${item.version} | ${item.license} | ${item.source} | ${item.licenseAssets.map((asset) => `\`${asset}\``).join("<br>")} |`,
    )
    .join("\n")}\n`,
);

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
