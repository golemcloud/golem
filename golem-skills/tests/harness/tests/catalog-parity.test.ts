import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { describe, it } from "node:test";
import yaml from "yaml";

const harnessRoot = process.cwd();
const skillsRoot = path.resolve(harnessRoot, "../../skills/effect");
const scenariosRoot = path.join(harnessRoot, "scenarios");
const languageKeys = new Set(["ts", "effect", "rust", "scala", "moonbit"]);

function findMissingEffectBranches(value: unknown, currentPath = "$root"): string[] {
  if (Array.isArray(value)) {
    return value.flatMap((entry, index) =>
      findMissingEffectBranches(entry, `${currentPath}[${index}]`),
    );
  }
  if (!value || typeof value !== "object") return [];

  const entries = Object.entries(value);
  const missing =
    entries.length > 0 && entries.every(([key]) => languageKeys.has(key)) && !("effect" in value)
      ? [currentPath]
      : [];
  return entries.reduce<string[]>(
    (paths, [key, entry]) =>
      paths.concat(findMissingEffectBranches(entry, `${currentPath}.${key}`)),
    missing,
  );
}

function collectStrings(value: unknown, strings: Set<string>): void {
  if (typeof value === "string") {
    strings.add(value);
  } else if (Array.isArray(value)) {
    value.forEach((entry) => collectStrings(entry, strings));
  } else if (value && typeof value === "object") {
    Object.values(value).forEach((entry) => collectStrings(entry, strings));
  }
}

describe("catalog Effect parity", () => {
  it("keeps Effect branches in every multi-language scenario map", async () => {
    const failures: string[] = [];
    for (const file of (await fs.readdir(scenariosRoot)).filter((name) => name.endsWith(".yaml"))) {
      const document = yaml.parse(await fs.readFile(path.join(scenariosRoot, file), "utf8"));
      const steps = [...(document.steps ?? []), ...(document.finally ?? [])];
      steps.forEach((step, index) => {
        if (step.only_if?.language && step.only_if.language !== "effect") return;
        if (step.skip_if?.language === "effect") return;
        failures.push(...findMissingEffectBranches(step, `${file}.steps[${index}]`));
      });
    }
    assert.deepEqual(failures, []);
  });

  it("keeps every canonical Effect skill covered by a scenario", async () => {
    const referenced = new Set<string>();
    for (const file of (await fs.readdir(scenariosRoot)).filter((name) => name.endsWith(".yaml"))) {
      collectStrings(
        yaml.parse(await fs.readFile(path.join(scenariosRoot, file), "utf8")),
        referenced,
      );
    }
    const skills = (await fs.readdir(skillsRoot, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
    assert.deepEqual(
      skills.filter((skill) => !referenced.has(skill)),
      [],
    );
  });

  it("retains Effect semantic requirements and atomic result parity", async () => {
    const scenariosWithRequirements: string[] = [];
    for (const file of (await fs.readdir(scenariosRoot)).filter((name) => name.endsWith(".yaml"))) {
      const document = yaml.parse(await fs.readFile(path.join(scenariosRoot, file), "utf8"));
      if (document.semanticRequirements !== undefined) {
        assert.ok(document.semanticRequirements.effect?.length > 0, file);
        scenariosWithRequirements.push(file);
      }
    }
    assert.ok(scenariosWithRequirements.length > 0);

    const atomic = yaml.parse(
      await fs.readFile(path.join(scenariosRoot, "atomic-block.yaml"), "utf8"),
    );
    const verification = atomic.steps.find(
      (step: { id?: string }) => step.id === "verify-event-sequence",
    );
    assert.deepEqual(verification.expect.effect, verification.expect.ts);
  });
});
