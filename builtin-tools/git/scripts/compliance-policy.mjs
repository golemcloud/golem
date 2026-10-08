import fs from "node:fs";
import path from "node:path";

const reviewedLicenseDeclarations = new Set([
  "(MIT AND BSD-3-Clause)",
  "(MIT AND Zlib)",
  "Apache-2.0",
  "BSD-3-Clause",
  "ISC",
  "MIT",
  "SEE LICENSE IN LICENSE",
]);

function normalizedNotice(file) {
  const lines = fs
    .readFileSync(file, "utf8")
    .replaceAll("\r\n", "\n")
    .split("\n");
  return `${lines.map((line) => line.trimEnd()).join("\n").trimEnd()}\n`;
}

function pakoZlibNotice(item) {
  if (item.version !== "1.0.11") {
    throw new Error(
      `${item.name}@${item.version} requires a reviewed supplemental notice rule`,
    );
  }
  const sourceFile = path.join(item.root, "lib/zlib/deflate.js");
  const source = fs.readFileSync(sourceFile, "utf8").replaceAll("\r\n", "\n");
  const match = source.match(
    /\/\/ \(C\) 1995-2013 Jean-loup Gailly and Mark Adler[\s\S]*?\/\/ 3\. This notice may not be removed or altered from any source distribution\./,
  );
  if (!match) {
    throw new Error(`missing reviewed Zlib source notice in ${sourceFile}`);
  }
  return `${match[0]
    .split("\n")
    .map((line) => line.replace(/^\/\/ ?/, ""))
    .join("\n")}\n`;
}

export function collectLicenseAssets(item, fallbackLicenseAssets) {
  if (!reviewedLicenseDeclarations.has(item.license)) {
    throw new Error(
      `${item.name}@${item.version} has unreviewed license declaration ${item.license}`,
    );
  }

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
  const assets = licenseFiles.length
    ? licenseFiles
    : fallbackLicenseAssets(item);

  if (item.name === "pako") {
    assets.push({ name: "ZLIB-LICENSE", content: pakoZlibNotice(item) });
  }

  return assets;
}
