import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { describe, it } from "node:test";
import yaml from "yaml";

const harnessRoot = process.cwd();
const allSkillsRoot = path.resolve(harnessRoot, "../../skills");
const skillsRoot = path.join(allSkillsRoot, "kotlin");
const scenariosRoot = path.join(harnessRoot, "scenarios");

interface ScenarioStep {
  only_if?: { language?: string | string[] };
  expectedSkills?: { kotlin?: unknown } | unknown[];
}

interface ScenarioDocument {
  skip_if?: { language?: string | string[] };
  steps?: ScenarioStep[];
}

async function loadScenarios(): Promise<Array<{ file: string; document: ScenarioDocument }>> {
  const files = (await fs.readdir(scenariosRoot)).filter((name) => name.endsWith(".yaml"));
  return Promise.all(
    files.map(async (file) => ({
      file,
      document: yaml.parse(await fs.readFile(path.join(scenariosRoot, file), "utf8")),
    })),
  );
}

function collectBranches(value: unknown, language: string, found: unknown[]): void {
  if (Array.isArray(value)) {
    value.forEach((entry) => collectBranches(entry, language, found));
  } else if (value && typeof value === "object") {
    for (const [key, entry] of Object.entries(value)) {
      if (key === language) found.push(entry);
      collectBranches(entry, language, found);
    }
  }
}

function collectKotlinBranches(value: unknown, found: unknown[]): void {
  collectBranches(value, "kotlin", found);
}

/**
 * A scenario that branches on no language at all applies to Kotlin as it stands, so it needs
 * neither a `kotlin` branch nor a skip. `languageAgnostic: true` says the same thing, but only
 * some such scenarios declare it.
 */
function branchesOnAnyLanguage(document: ScenarioDocument): boolean {
  return ["rust", "ts", "effect", "scala", "moonbit", "go"].some((language) => {
    const found: unknown[] = [];
    collectBranches(document.steps, language, found);
    return found.length > 0;
  });
}

function namesKotlin(language: string | string[] | undefined): boolean {
  return Array.isArray(language) ? language.includes("kotlin") : language === "kotlin";
}

function hasKotlinSkip(skipIf: ScenarioDocument["skip_if"]): boolean {
  return namesKotlin(skipIf?.language);
}

/**
 * A scenario whose languages each get their own step covers Kotlin through
 * `only_if: language: kotlin` rather than through a `kotlin` branch inside a shared step.
 */
function hasKotlinGatedStep(steps: ScenarioStep[]): boolean {
  return steps.some((step) => namesKotlin(step.only_if?.language));
}

/**
 * Every skill a scenario asserts for Kotlin: a `kotlin` branch of `expectedSkills`, or the flat
 * `expectedSkills` list of a scenario whose steps are gated on Kotlin. `allowedExtraSkills` does
 * not count -- it tolerates a skill, it does not require it.
 */
function collectAssertedKotlinSkills(document: ScenarioDocument): string[] {
  const asserted: string[] = [];
  const kotlinGated = hasKotlinGatedStep(document.steps ?? []);
  for (const step of document.steps ?? []) {
    const expected = step.expectedSkills;
    if (Array.isArray(expected)) {
      if (kotlinGated) asserted.push(...expected.filter((skill) => typeof skill === "string"));
    } else {
      const branch = expected?.kotlin;
      if (Array.isArray(branch)) asserted.push(...branch.filter((skill) => typeof skill === "string"));
    }
  }
  return asserted;
}

/**
 * Kotlin skills that no scenario asserts yet. Permission cards are tested for no language but
 * Effect, whose `effect-phase8-capabilities` scenario is Effect-only by construction. The
 * assertion below is exact in both directions, so covering this skill means deleting it from this
 * list rather than leaving a stale entry behind.
 */
const unassertedKotlinSkills = ["golem-permission-card-kotlin"];

describe("Kotlin scenario coverage", () => {
  it("gives every scenario either a Kotlin branch or a Kotlin skip", async () => {
    const missing: string[] = [];
    for (const { file, document } of await loadScenarios()) {
      const branches: unknown[] = [];
      collectKotlinBranches(document.steps, branches);
      if (
        branches.length === 0 &&
        branchesOnAnyLanguage(document) &&
        !hasKotlinGatedStep(document.steps ?? []) &&
        !hasKotlinSkip(document.skip_if)
      ) {
        missing.push(file);
      }
    }
    assert.deepEqual(missing, []);
  });

  it("asserts every Kotlin skill except the recorded gaps", async () => {
    const asserted = new Set<string>();
    for (const { document } of await loadScenarios()) {
      if (hasKotlinSkip(document.skip_if)) continue;
      collectAssertedKotlinSkills(document).forEach((skill) => asserted.add(skill));
    }
    const missing = (await fs.readdir(skillsRoot, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory() && !asserted.has(entry.name))
      .map((entry) => entry.name)
      .sort();
    assert.deepEqual(missing, [...unassertedKotlinSkills].sort());
  });

  it("only expects Kotlin skills that exist", async () => {
    // A Kotlin branch may name a language-agnostic skill from `skills/common` beside its own,
    // so a skill is resolved against every language's directory, not just `skills/kotlin`.
    const skills = new Set<string>();
    for (const group of await fs.readdir(allSkillsRoot, { withFileTypes: true })) {
      if (!group.isDirectory()) continue;
      for (const entry of await fs.readdir(path.join(allSkillsRoot, group.name), {
        withFileTypes: true,
      })) {
        if (entry.isDirectory()) skills.add(entry.name);
      }
    }
    const unknown: string[] = [];
    for (const { file, document } of await loadScenarios()) {
      for (const step of document.steps ?? []) {
        const expected = Array.isArray(step.expectedSkills)
          ? undefined
          : step.expectedSkills?.kotlin;
        for (const skill of Array.isArray(expected) ? expected : []) {
          if (!skills.has(skill)) unknown.push(`${file}: ${skill}`);
        }
      }
    }
    assert.deepEqual(unknown, []);
  });

  it("keeps each Kotlin skill's name equal to its directory", async () => {
    const mismatched: string[] = [];
    for (const entry of await fs.readdir(skillsRoot, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const text = await fs.readFile(path.join(skillsRoot, entry.name, "SKILL.md"), "utf8");
      const match = /^name:\s*(\S+)\s*$/m.exec(text);
      if (match?.[1] !== entry.name) mismatched.push(entry.name);
    }
    assert.deepEqual(mismatched, []);
  });
});
