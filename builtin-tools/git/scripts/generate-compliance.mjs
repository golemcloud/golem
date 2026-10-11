import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { collectLicenseAssets } from "./compliance-policy.mjs";

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
  return `${lines
    .map((line) => line.trimEnd())
    .join("\n")
    .trimEnd()}\n`;
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
        content: stripLineCommentNotice(path.join(item.root, "diff3.js"), 22),
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
    `${left.name}@${left.version}`.localeCompare(
      `${right.name}@${right.version}`,
    ),
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
    packages: packages
      .map((item, index) => ({
        name: item.name,
        SPDXID: `SPDXRef-Package-${index + 1}`,
        versionInfo: item.version,
        downloadLocation: item.source,
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: item.license,
        copyrightText: "NOASSERTION",
      }))
      .concat({
        name: provenance.xdiff.name,
        SPDXID: "SPDXRef-Package-Xdiff",
        versionInfo: provenance.xdiff.version,
        downloadLocation: provenance.xdiff.sourceArchive,
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: provenance.xdiff.license,
        copyrightText: "See xdiff source headers in the license bundle",
        checksums: [
          {
            algorithm: "SHA256",
            checksumValue: provenance.xdiff.sourceArchiveSha256,
          },
        ],
      }),
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
    normalizedNotice(
      path.join(root, "../../sdks/ts/packages/golem-ts-sdk/LICENSE"),
    ),
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
    `# Git tool source and notices\n\nThe Git object and worktree implementation is provided by pinned upstream packages. Golem-owned wrappers expose local and HTTP(S) workflows and integrate them with the component filesystem. The isomorphic-git source changes are preserved in the source repository's checksum-pinned transfer patch.\n\n${sourceLines.join("\n")}\n\n- ${provenance.xdiff.name} ${provenance.xdiff.version}: ${provenance.xdiff.sourceTag}\n  - Source archive: ${provenance.xdiff.sourceArchive}\n  - Source SHA-256: \`${provenance.xdiff.sourceArchiveSha256}\`\n  - License: ${provenance.xdiff.license}\n  - Preferred source, original notices, build glue and rebuilding instructions: \`git/xdiff/\` in the license bundle.\n\nThe Git and upstream package names are used descriptively. No upstream logos are used, no endorsement is implied, and the respective trademarks belong to their owners. Production publication requires legal and release-owner approval, including review of the xdiff library's LGPL distribution obligations.\n`,
  ],
]);

const archive = path.join(root, "golem-temp/xdiff/git-source.tar.xz");
const sourceDigest = crypto
  .createHash("sha256")
  .update(fs.readFileSync(archive))
  .digest("hex");
if (sourceDigest !== provenance.xdiff.sourceArchiveSha256)
  throw new Error("xdiff preferred-source checksum mismatch");
function sourceArchive(args) {
  const result = spawnSync("tar", args, { encoding: "utf8" });
  if (result.error || result.status !== 0)
    throw (
      result.error ??
      new Error(result.stderr || "xdiff source extraction failed")
    );
  return result.stdout;
}
const prefix = `git-${provenance.xdiff.version}/`;
const preferredSources = sourceArchive(["-tJf", archive])
  .split("\n")
  .filter((file) => file.startsWith(`${prefix}xdiff/`) && /\.[ch]$/.test(file));
if (preferredSources.length !== 15)
  throw new Error(
    "xdiff preferred-source file set differs from the pinned archive",
  );
for (const file of preferredSources)
  generated.set(
    `git/xdiff/source/${path.basename(file)}`,
    sourceArchive(["-xJOf", archive, file]),
  );
for (const file of ["LGPL-2.1", "COPYING"])
  generated.set(
    `git/xdiff/${file}`,
    sourceArchive(["-xJOf", archive, `${prefix}${file}`]),
  );
for (const file of ["git-compat-util.h", "normalized-diff.c"])
  generated.set(
    `git/xdiff/source/${file}`,
    fs.readFileSync(path.join(root, "xdiff", file), "utf8"),
  );
generated.set(
  "git/xdiff/build-xdiff.mjs",
  fs.readFileSync(path.join(root, "scripts/build-xdiff.mjs"), "utf8"),
);
generated.set(
  "git/xdiff/provenance.json",
  `${JSON.stringify(provenance.xdiff, null, 2)}\n`,
);
generated.set("git/xdiff/GOLEM-GLUE-APACHE-2.0", canonicalApacheLicense());
generated.set(
  "git/xdiff/README.md",
  `# Git xdiff source and rebuilding\n\nThis bundle preserves the preferred C source of the Git ${provenance.xdiff.version} xdiff library, including each original copyright/license header, the LGPL-2.1 license and its referenced GPL text. The histogram implementation includes its full EDL-1.0 notice in source/xhistogram.c. Golem's compatibility header and normalized-diff.c are Apache-2.0 licensed. Upstream xdiff source is unmodified; these two glue files supply an import-free bounded allocator and normalized output callback.\n\nThe component executes transpiled JavaScript, not a second WASM module or native Git subprocess. build-xdiff.mjs and provenance.json preserve the exact toolchain and transformation.\n\nTo rebuild or replace this library in the tool, obtain the golem source commit recorded in the release provenance, install WASI SDK 25 (clang 19.1.5-wasi-sdk), set WASI_SDK_PATH to its directory, and run npm ci --ignore-scripts in builtin-tools/git. Run npm run build:git-library, then the focused cargo make build-git-tool task from the repository root. The latter bundles the regenerated library and rebuilds the component through its Golem manifest. Generated output lives in golem-temp/xdiff and is not committed.\n\nFor a modified xdiff library, modify the checksum-verified extracted C source and/or the two glue files, adapt build-xdiff.mjs to use that modified source, and update the expected generatedJavaScriptSha256 after inspecting the result. The source archive, compiler and generated-output checks are reproducibility checks, not a prohibition on replacing the library. Rebuild the tool as above to relink it. Preserve modified source and its applicable notices alongside any redistributed component.\n`,
);

for (const item of packages) {
  const assets = collectLicenseAssets(item, fallbackLicenseAssets);
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
    .join(
      "\n",
    )}\n| ${provenance.xdiff.name} | ${provenance.xdiff.version} | ${provenance.xdiff.license} | ${provenance.xdiff.sourceArchive} | \`xdiff/\` (preferred source, license texts, notices and rebuild instructions) |\n`,
);

let stale = false;
for (const [relative, content] of generated) {
  const target = path.join(outputRoot, relative);
  if (check) {
    if (!fs.existsSync(target) || fs.readFileSync(target, "utf8") !== content) {
      console.error(
        `stale compliance artifact: ${path.relative(root, target)}`,
      );
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
  console.log(
    `${check ? "verified" : "generated"} compliance bundle ${digest}`,
  );
}
