import { spawn } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { isDeepStrictEqual, parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import * as yaml from "yaml";
import { z } from "zod";

const LANGUAGE_KEYS = new Set(["ts", "effect", "rust", "scala", "moonbit"]);
const EXPECTED_UNIT_COUNT = 38;

const AttemptSchema = z.object({
  number: z.number().int().positive(),
  startedAt: z.string(),
  finishedAt: z.string().optional(),
  thread: z.string().optional(),
  outcome: z.enum(["editing", "passed", "failed", "blocked"]),
  reports: z.array(z.string()).default([]),
  error: z.string().optional(),
});

const EvidenceSchema = z.object({
  verifiedAt: z.string(),
  effectGolemRef: z.string(),
  thread: z.string().optional(),
  reports: z.array(z.string()),
});

const UnitSchema = z.object({
  id: z.string(),
  targetSkill: z.string(),
  references: z.array(z.string()).optional(),
  verifyScenarios: z.array(z.string()).min(1),
  relatedScenarios: z.array(z.string()).default([]),
  prerequisites: z.array(z.string()).default([]),
  strategy: z.enum(["direct", "adapted"]),
  notes: z.string().optional(),
  state: z.enum(["pending", "editing", "verifying", "passed", "blocked"]),
  attempts: z.array(AttemptSchema).default([]),
  evidence: EvidenceSchema.nullable().default(null),
});

const ManifestSchema = z.object({
  version: z.literal(1),
  effectGolem: z.object({
    repository: z.string().url(),
    ref: z.string(),
    localPathEnv: z.string().default("GOLEM_EFFECT_GOLEM_PATH"),
  }),
  policy: z.object({
    maxAttempts: z.number().int().positive().default(3),
    stopOnBlocked: z.boolean().default(true),
    workerMode: z.enum(["low", "medium", "high", "ultra"]).default("high"),
    buildProfile: z.enum(["debug", "release"]).default("release"),
  }),
  units: z.array(UnitSchema),
});

export type MigrationManifest = z.infer<typeof ManifestSchema>;
export type MigrationUnit = MigrationManifest["units"][number];
type MigrationAttempt = MigrationUnit["attempts"][number];

interface CommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

interface AmpResult extends CommandResult {
  thread?: string;
  result?: string;
}

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string };
  status: "pass" | "fail";
  artifactPaths?: string[];
  results: Array<{
    success: boolean;
    error?: string;
    expectedSkills?: string[];
    activatedSkills?: string[];
  }>;
}

interface RunOptions {
  repoRoot: string;
  harnessRoot: string;
  manifestPath: string;
  commitEach: boolean;
  verifyOnly: boolean;
}

class ScopeViolationError extends Error {}
class ControllerError extends Error {}

function timestamp(): string {
  return new Date().toISOString();
}

async function runCommand(
  command: string,
  args: string[],
  cwd: string,
  options: { env?: NodeJS.ProcessEnv; echo?: boolean } = {},
): Promise<CommandResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env: { ...process.env, ...options.env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";

    child.stdout.on("data", (data: Buffer) => {
      const text = data.toString();
      stdout += text;
      if (options.echo) process.stdout.write(text);
    });
    child.stderr.on("data", (data: Buffer) => {
      const text = data.toString();
      stderr += text;
      if (options.echo) process.stderr.write(text);
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ exitCode: code ?? 1, stdout, stderr }));
  });
}

function commandError(command: string, args: string[], result: CommandResult): Error {
  const output = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
  return new Error(`${command} ${args.join(" ")} failed (${result.exitCode})\n${output}`);
}

async function runRequiredCommand(
  command: string,
  args: string[],
  cwd: string,
  options: { env?: NodeJS.ProcessEnv; echo?: boolean } = {},
): Promise<CommandResult> {
  const result = await runCommand(command, args, cwd, options);
  if (result.exitCode !== 0) throw commandError(command, args, result);
  return result;
}

export async function loadManifest(manifestPath: string): Promise<MigrationManifest> {
  const content = await fs.readFile(manifestPath, "utf8");
  const parsed = yaml.parse(content) as unknown;
  const manifest = ManifestSchema.parse(parsed);
  validateManifestInventory(manifest);
  return manifest;
}

async function saveManifest(manifestPath: string, manifest: MigrationManifest): Promise<void> {
  const content = yaml.stringify(manifest, { lineWidth: 0 });
  const temporaryPath = `${manifestPath}.tmp`;
  await fs.writeFile(temporaryPath, content, "utf8");
  await fs.rename(temporaryPath, manifestPath);
}

export function validateManifestInventory(manifest: MigrationManifest): void {
  if (manifest.units.length !== EXPECTED_UNIT_COUNT) {
    throw new Error(
      `Effect migration manifest must contain ${EXPECTED_UNIT_COUNT} units, found ${manifest.units.length}`,
    );
  }

  const ids = new Set<string>();
  const targets = new Set<string>();
  for (const unit of manifest.units) {
    if (ids.has(unit.id)) throw new Error(`Duplicate migration unit id: ${unit.id}`);
    if (targets.has(unit.targetSkill)) {
      throw new Error(`Duplicate target skill: ${unit.targetSkill}`);
    }
    ids.add(unit.id);
    targets.add(unit.targetSkill);

    if (unit.targetSkill !== "golem-add-npm-package" && !unit.targetSkill.endsWith("-effect")) {
      throw new Error(`Effect skill must end in -effect: ${unit.targetSkill}`);
    }
  }

  for (const unit of manifest.units) {
    for (const prerequisite of unit.prerequisites) {
      if (!ids.has(prerequisite)) {
        throw new Error(`${unit.id} has unknown prerequisite ${prerequisite}`);
      }
    }
  }

  const visited = new Set<string>();
  const visiting = new Set<string>();
  const visit = (unit: MigrationUnit) => {
    if (visiting.has(unit.id)) throw new Error(`Prerequisite cycle includes ${unit.id}`);
    if (visited.has(unit.id)) return;
    visiting.add(unit.id);
    for (const prerequisite of unit.prerequisites) {
      visit(manifest.units.find(({ id }) => id === prerequisite)!);
    }
    visiting.delete(unit.id);
    visited.add(unit.id);
  };
  manifest.units.forEach(visit);
}

export async function validateManifestFiles(
  repoRoot: string,
  manifest: MigrationManifest,
): Promise<void> {
  const referenceOwners = new Map<string, MigrationUnit>();
  for (const unit of manifest.units) {
    for (const reference of unitReferences(unit)) {
      await fs.access(path.join(repoRoot, reference));
      const existingOwner = referenceOwners.get(reference);
      if (existingOwner) {
        throw new Error(`${reference} is referenced by both ${existingOwner.id} and ${unit.id}`);
      }
      referenceOwners.set(reference, unit);
    }
    for (const scenario of unitScenarioNames(unit)) {
      await fs.access(
        path.join(repoRoot, "golem-skills", "tests", "harness", "scenarios", `${scenario}.yaml`),
      );
    }
  }

  const sourceSkillOwners = new Map<string, MigrationUnit>();
  for (const [reference, unit] of referenceOwners) {
    sourceSkillOwners.set(path.basename(path.dirname(reference)), unit);
  }

  const unreferencedSkills: string[] = [];
  for (const language of ["ts", "rust", "scala", "moonbit"]) {
    const languageRoot = path.join(repoRoot, "golem-skills", "skills", language);
    const entries = await fs.readdir(languageRoot, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const reference = path.posix.join("golem-skills", "skills", language, entry.name, "SKILL.md");
      try {
        await fs.access(path.join(repoRoot, reference));
      } catch {
        continue;
      }
      if (!referenceOwners.has(reference)) unreferencedSkills.push(reference);
    }
  }
  if (unreferencedSkills.length > 0) {
    throw new Error(
      `Language-specific skills missing from the Effect migration manifest:\n${unreferencedSkills.map((skill) => `  - ${skill}`).join("\n")}`,
    );
  }

  const scenariosRoot = path.join(repoRoot, "golem-skills", "tests", "harness", "scenarios");
  const verificationScenarios = new Set(manifest.units.flatMap((unit) => unit.verifyScenarios));
  const unmappedReferences: string[] = [];
  const unownedScenarios: string[] = [];
  for (const entry of await fs.readdir(scenariosRoot, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(".yaml")) continue;
    const scenarioName = entry.name.slice(0, -".yaml".length);
    const document = yaml.parse(
      await fs.readFile(path.join(scenariosRoot, entry.name), "utf8"),
    ) as unknown;
    const referencedUnits = new Set<MigrationUnit>();
    const visit = (value: unknown): void => {
      if (typeof value === "string") {
        const unit = sourceSkillOwners.get(value);
        if (unit) referencedUnits.add(unit);
      } else if (Array.isArray(value)) {
        value.forEach(visit);
      } else if (value && typeof value === "object") {
        Object.values(value).forEach(visit);
      }
    };
    visit(document);

    for (const unit of referencedUnits) {
      if (!unitScenarioNames(unit).includes(scenarioName)) {
        unmappedReferences.push(`${scenarioName} -> ${unit.id}`);
      }
    }
    if (referencedUnits.size > 0 && !verificationScenarios.has(scenarioName)) {
      unownedScenarios.push(scenarioName);
    }
  }
  if (unmappedReferences.length > 0) {
    throw new Error(
      `Language-specific scenario references missing from the Effect migration manifest:\n${unmappedReferences.map((reference) => `  - ${reference}`).join("\n")}`,
    );
  }
  if (unownedScenarios.length > 0) {
    throw new Error(
      `Language-specific scenarios without a live verification owner:\n${unownedScenarios.map((scenario) => `  - ${scenario}`).join("\n")}`,
    );
  }
}

export function selectNextUnit(
  manifest: MigrationManifest,
  requestedUnit?: string,
): MigrationUnit | undefined {
  if (requestedUnit) {
    const unit = manifest.units.find(({ id }) => id === requestedUnit);
    if (!unit) throw new Error(`Unknown migration unit: ${requestedUnit}`);
    return unit;
  }

  const active = manifest.units.find(
    (unit) =>
      (unit.state === "editing" || unit.state === "verifying") &&
      unit.attempts.length < manifest.policy.maxAttempts,
  );
  if (active) return active;

  return manifest.units.find(
    (unit) =>
      unit.state === "pending" &&
      unit.prerequisites.every(
        (id) => manifest.units.find((candidate) => candidate.id === id)?.state === "passed",
      ),
  );
}

function unitReferences(unit: MigrationUnit): string[] {
  if (unit.references) return unit.references;
  return ["ts", "rust", "scala", "moonbit"].map(
    (language) => `golem-skills/skills/${language}/${unit.id}-${language}/SKILL.md`,
  );
}

function unitScenarioNames(unit: MigrationUnit): string[] {
  return [...new Set([...unit.verifyScenarios, ...unit.relatedScenarios])];
}

function unitAllowedPaths(unit: MigrationUnit, manifestRelativePath: string): string[] {
  return [
    manifestRelativePath,
    `golem-skills/skills/effect/${unit.targetSkill}`,
    ...unitScenarioNames(unit).map(
      (scenario) => `golem-skills/tests/harness/scenarios/${scenario}.yaml`,
    ),
  ];
}

async function changedPaths(repoRoot: string): Promise<string[]> {
  const [tracked, untracked] = await Promise.all([
    runRequiredCommand("git", ["diff", "HEAD", "--name-only", "--"], repoRoot),
    runRequiredCommand("git", ["ls-files", "--others", "--exclude-standard", "--"], repoRoot),
  ]);
  return [...new Set([...tracked.stdout.split("\n"), ...untracked.stdout.split("\n")])].filter(
    Boolean,
  );
}

function pathIsAllowed(candidate: string, allowed: string[]): boolean {
  return allowed.some((entry) => candidate === entry || candidate.startsWith(`${entry}/`));
}

async function assertScopedChanges(
  repoRoot: string,
  unit: MigrationUnit,
  manifestPath: string,
): Promise<void> {
  const manifestRelativePath = path.relative(repoRoot, manifestPath);
  const allowed = unitAllowedPaths(unit, manifestRelativePath);
  const unexpected = (await changedPaths(repoRoot)).filter(
    (candidate) => !pathIsAllowed(candidate, allowed),
  );
  if (unexpected.length > 0) {
    throw new ScopeViolationError(
      `Out-of-scope changes detected:\n${unexpected.map((p) => `  - ${p}`).join("\n")}`,
    );
  }
}

function isLanguageMap(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  return keys.length > 0 && keys.every((key) => LANGUAGE_KEYS.has(key));
}

export function findMissingEffectBranches(value: unknown, currentPath = "$root"): string[] {
  const missing: string[] = [];
  if (Array.isArray(value)) {
    value.forEach((entry, index) => {
      missing.push(...findMissingEffectBranches(entry, `${currentPath}[${index}]`));
    });
    return missing;
  }
  if (!value || typeof value !== "object") return missing;

  if (isLanguageMap(value) && !("effect" in value)) {
    missing.push(currentPath);
  }
  for (const [key, entry] of Object.entries(value)) {
    missing.push(...findMissingEffectBranches(entry, `${currentPath}.${key}`));
  }
  return missing;
}

function scenarioExcludesEffect(step: Record<string, unknown>): boolean {
  return scenarioExcludesLanguage(step, "effect");
}

function scenarioExcludesLanguage(step: Record<string, unknown>, language: string): boolean {
  const onlyIf = step.only_if as Record<string, unknown> | undefined;
  const skipIf = step.skip_if as Record<string, unknown> | undefined;
  return (
    (typeof onlyIf?.language === "string" && onlyIf.language !== language) ||
    skipIf?.language === language
  );
}

function resolveScenarioForLanguage(value: unknown, language: string): unknown {
  if (Array.isArray(value)) {
    return value
      .filter(
        (entry) =>
          !entry ||
          typeof entry !== "object" ||
          !scenarioExcludesLanguage(entry as Record<string, unknown>, language),
      )
      .map((entry) => resolveScenarioForLanguage(entry, language));
  }
  if (!value || typeof value !== "object") return value;
  if (isLanguageMap(value)) {
    return resolveScenarioForLanguage(value[language], language);
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, entry]) => [key, resolveScenarioForLanguage(entry, language)]),
  );
}

async function loadCommittedScenario(
  repoRoot: string,
  scenarioRelativePath: string,
): Promise<Record<string, unknown> | undefined> {
  const result = await runCommand("git", ["show", `HEAD:${scenarioRelativePath}`], repoRoot);
  if (result.exitCode !== 0) return undefined;
  return yaml.parse(result.stdout) as Record<string, unknown>;
}

async function validateNonEffectScenariosUnchanged(
  repoRoot: string,
  scenarioRelativePath: string,
  current: Record<string, unknown>,
): Promise<void> {
  const committed = await loadCommittedScenario(repoRoot, scenarioRelativePath);
  if (!committed) return;

  for (const language of ["ts", "rust", "scala", "moonbit"]) {
    const before = resolveScenarioForLanguage(committed, language);
    const after = resolveScenarioForLanguage(current, language);
    if (!isDeepStrictEqual(before, after)) {
      throw new Error(
        `${scenarioRelativePath} changes existing non-Effect (${language}) scenario behavior; only Effect branches may be added`,
      );
    }
  }
}

function scenarioExpectedSkills(document: Record<string, unknown>): unknown[] {
  const steps = (document.steps as unknown[] | undefined) ?? [];
  return steps
    .filter((step): step is Record<string, unknown> => Boolean(step && typeof step === "object"))
    .filter((step) => !scenarioExcludesEffect(step))
    .map((step) => step.expectedSkills)
    .filter((value) => value !== undefined);
}

export async function validateUnitStatic(repoRoot: string, unit: MigrationUnit): Promise<void> {
  const skillPath = path.join(
    repoRoot,
    "golem-skills",
    "skills",
    "effect",
    unit.targetSkill,
    "SKILL.md",
  );
  const skillContent = await fs.readFile(skillPath, "utf8");
  const frontmatterMatch = /^---\n([\s\S]*?)\n---\n/.exec(skillContent);
  if (!frontmatterMatch) throw new Error(`${skillPath} has no YAML frontmatter`);
  const frontmatter = yaml.parse(frontmatterMatch[1]) as Record<string, unknown>;
  if (frontmatter.name !== unit.targetSkill) {
    throw new Error(`Skill frontmatter name must be ${unit.targetSkill}`);
  }
  if (typeof frontmatter.description !== "string" || frontmatter.description.trim() === "") {
    throw new Error(`${skillPath} frontmatter must have a non-empty description`);
  }

  let expectsTargetSkill = false;
  for (const scenarioName of unitScenarioNames(unit)) {
    const scenarioRelativePath = path.join(
      "golem-skills",
      "tests",
      "harness",
      "scenarios",
      `${scenarioName}.yaml`,
    );
    const scenarioPath = path.join(repoRoot, scenarioRelativePath);
    const document = yaml.parse(await fs.readFile(scenarioPath, "utf8")) as Record<string, unknown>;
    await validateNonEffectScenariosUnchanged(repoRoot, scenarioRelativePath, document);
    const steps = [
      ...((document.steps as unknown[] | undefined) ?? []),
      ...((document.finally as unknown[] | undefined) ?? []),
    ];
    const missingBranches = steps
      .filter((step): step is Record<string, unknown> => Boolean(step && typeof step === "object"))
      .filter((step) => !scenarioExcludesEffect(step))
      .flatMap((step, index) => findMissingEffectBranches(step, `${scenarioName}.steps[${index}]`));
    if (missingBranches.length > 0) {
      throw new Error(
        `${scenarioName} has language maps without effect branches:\n${missingBranches.map((p) => `  - ${p}`).join("\n")}`,
      );
    }

    for (const expected of scenarioExpectedSkills(document)) {
      if (Array.isArray(expected) && expected.includes(unit.targetSkill)) expectsTargetSkill = true;
      if (
        expected &&
        typeof expected === "object" &&
        Array.isArray((expected as Record<string, unknown>).effect) &&
        ((expected as Record<string, unknown>).effect as unknown[]).includes(unit.targetSkill)
      ) {
        expectsTargetSkill = true;
      }
    }
  }
  if (!expectsTargetSkill) {
    throw new Error(`No mapped scenario expects ${unit.targetSkill} for Effect`);
  }
}

async function buildWorkerPrompt(
  repoRoot: string,
  manifest: MigrationManifest,
  unit: MigrationUnit,
  previousAttempt?: MigrationAttempt,
): Promise<string> {
  const basePrompt = await fs.readFile(
    path.join(repoRoot, "golem-skills", "effect-migration-worker.md"),
    "utf8",
  );
  const references = unitReferences(unit);
  const scenarios = unitScenarioNames(unit).map(
    (name) => `golem-skills/tests/harness/scenarios/${name}.yaml`,
  );
  const prerequisiteSkills = unit.prerequisites.map((id) => {
    const prerequisite = manifest.units.find((candidate) => candidate.id === id)!;
    return `golem-skills/skills/effect/${prerequisite.targetSkill}/SKILL.md`;
  });
  const localSdkPath = process.env[manifest.effectGolem.localPathEnv];
  const previousFailure = previousAttempt?.error;
  const artifactPaths = previousAttempt
    ? await collectAttemptArtifactPaths(repoRoot, previousAttempt)
    : [];

  return `${basePrompt}

## Assignment

- Unit: ${unit.id}
- Target skill: golem-skills/skills/effect/${unit.targetSkill}/SKILL.md
- Strategy: ${unit.strategy}${unit.notes ? ` — ${unit.notes}` : ""}
- Effect SDK: ${manifest.effectGolem.repository} at ${manifest.effectGolem.ref}
${localSdkPath ? `- Local Effect SDK checkout: ${localSdkPath}` : "- Use Librarian to inspect the Effect SDK repository."}
- Existing skill references:
${references.map((reference) => `  - ${reference}`).join("\n")}
- Scenarios you may edit:
${scenarios.map((scenario) => `  - ${scenario}`).join("\n")}
- Passed prerequisite Effect skills:
${prerequisiteSkills.length > 0 ? prerequisiteSkills.map((skill) => `  - ${skill}`).join("\n") : "  - none"}
- Canary reference: golem-skills/skills/effect/golem-add-agent-effect/SKILL.md
- Generated Effect guide: cli/golem-cli/templates/effect/common/AGENTS.md
${previousFailure ? buildPreviousFailureSection(previousFailure, previousAttempt.reports, artifactPaths) : ""}
Implement this one unit now. Read other files as needed, but edit only the target skill and listed
scenarios.`;
}

async function collectAttemptArtifactPaths(
  repoRoot: string,
  attempt: MigrationAttempt,
): Promise<string[]> {
  const artifactPaths = new Set<string>();
  for (const reportPath of attempt.reports) {
    try {
      const report = JSON.parse(
        await fs.readFile(path.resolve(repoRoot, reportPath), "utf8"),
      ) as ScenarioReport;
      for (const artifactPath of report.artifactPaths ?? []) {
        const relativePath = path.relative(repoRoot, artifactPath);
        artifactPaths.add(relativePath.startsWith("..") ? artifactPath : relativePath);
      }
    } catch {
      // The report path and original failure still give the repair worker useful evidence.
    }
  }
  return [...artifactPaths];
}

export function buildPreviousFailureSection(
  failure: string,
  reports: string[],
  artifactPaths: string[],
): string {
  const list = (paths: string[]) =>
    paths.length > 0 ? paths.map((entry) => `  - ${entry}`).join("\n") : "  - none";

  return `
## Previous live verification failed — repair it

The controller ran the real Amp × Effect scenarios after the prior edit. Diagnose the failure
before changing the skill again. Inspect the generated application and the complete report; the
short error alone may hide a runtime trap or incorrect generated code.

- Reports:
${list(reports)}
- Generated scenario workspaces and artifacts:
${list(artifactPaths)}

Use generated workspaces only as diagnostic evidence. Fix the assigned Effect skill and/or its
listed scenarios, not the generated application. Do not merely repeat the previous implementation.

### Failure

${failure}
`;
}

export function ampWorkerArgs(mode: string, prompt: string, thread?: string): string[] {
  const executeArgs = [
    "--mode",
    mode,
    "--dangerously-allow-all",
    "--execute",
    prompt,
    "--stream-json",
  ];
  return thread ? ["threads", "continue", thread, ...executeArgs] : executeArgs;
}

async function runAmpWorker(
  repoRoot: string,
  mode: string,
  prompt: string,
  continueThread?: string,
): Promise<AmpResult> {
  return new Promise((resolve, reject) => {
    const args = ampWorkerArgs(mode, prompt, continueThread);
    const child = spawn("amp", args, {
      cwd: repoRoot,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let buffered = "";
    let thread: string | undefined;
    let resultText: string | undefined;

    child.stdout.on("data", (data: Buffer) => {
      const text = data.toString();
      stdout += text;
      buffered += text;
      const lines = buffered.split("\n");
      buffered = lines.pop() ?? "";
      for (const line of lines) {
        if (!line.trim()) continue;
        try {
          const message = JSON.parse(line) as Record<string, unknown>;
          if (typeof message.session_id === "string") thread = message.session_id;
          if (message.type === "assistant") {
            const content = (message.message as Record<string, unknown> | undefined)?.content;
            if (Array.isArray(content)) {
              for (const block of content) {
                if (
                  block &&
                  typeof block === "object" &&
                  (block as Record<string, unknown>).type === "text" &&
                  typeof (block as Record<string, unknown>).text === "string"
                ) {
                  process.stdout.write(`${(block as Record<string, unknown>).text}\n`);
                }
              }
            }
          }
          if (message.type === "result") {
            if (typeof message.result === "string") resultText = message.result;
            if (typeof message.error === "string") resultText = message.error;
          }
        } catch {
          process.stdout.write(line + "\n");
        }
      }
    });
    child.stderr.on("data", (data: Buffer) => {
      const text = data.toString();
      stderr += text;
      process.stderr.write(text);
    });
    child.on("error", reject);
    child.on("close", (code) => {
      resolve({
        exitCode: code ?? 1,
        stdout,
        stderr,
        thread: thread ? `https://ampcode.com/threads/${thread}` : continueThread,
        result: resultText,
      });
    });
  });
}

async function cargoTargetDirectory(repoRoot: string): Promise<string> {
  const result = await runRequiredCommand(
    "cargo",
    ["metadata", "--no-deps", "--format-version", "1"],
    repoRoot,
  );
  const metadata = JSON.parse(result.stdout) as { target_directory?: string };
  if (!metadata.target_directory) throw new Error("cargo metadata returned no target_directory");
  return metadata.target_directory;
}

async function buildGolem(repoRoot: string, profile: "debug" | "release"): Promise<string> {
  const args = ["build", "-p", "golem"];
  if (profile === "release") args.push("--release");
  await runRequiredCommand("cargo", args, repoRoot, { echo: true });
  return path.join(await cargoTargetDirectory(repoRoot), profile);
}

async function runScenario(
  options: RunOptions,
  unit: MigrationUnit,
  attempt: MigrationAttempt,
  scenario: string,
  targetDir: string,
): Promise<{ reportPath: string; report: ScenarioReport }> {
  const outputRelative = path.join(
    "results",
    "effect-migration",
    unit.id,
    `attempt-${attempt.number}`,
    scenario,
  );
  const output = path.join(options.harnessRoot, outputRelative);
  const env = {
    GOLEM_PATH: options.repoRoot,
    GOLEM_TARGET_DIR: targetDir,
  };
  const tsx = path.join(options.harnessRoot, "node_modules", ".bin", "tsx");
  const args = [
    "src/run.ts",
    "--agent",
    "amp",
    "--language",
    "effect",
    "--scenario",
    scenario,
    "--retries",
    "1",
    "--output",
    outputRelative,
  ];
  const result = await runCommand(tsx, args, options.harnessRoot, { env, echo: true });
  const reportPath = path.join(output, `amp-effect-${scenario}.json`);
  let report: ScenarioReport;
  try {
    report = JSON.parse(await fs.readFile(reportPath, "utf8")) as ScenarioReport;
  } catch (error) {
    if (result.exitCode !== 0) throw commandError(tsx, args, result);
    throw error;
  }
  return { reportPath: path.relative(options.repoRoot, reportPath), report };
}

export function reportFailure(report: ScenarioReport, targetSkill: string): string | undefined {
  if (report.matrix.agent !== "amp" || report.matrix.language !== "effect") {
    return `Unexpected report matrix: ${JSON.stringify(report.matrix)}`;
  }
  if (report.status === "pass") {
    const activationVerified = report.results.some(
      ({ expectedSkills, activatedSkills }) =>
        expectedSkills?.includes(targetSkill) && activatedSkills?.includes(targetSkill),
    );
    if (!activationVerified) {
      return `Report has no activation evidence for ${targetSkill}`;
    }
    return undefined;
  }
  const failures = report.results
    .filter(({ success }) => !success)
    .map(({ error }) => error ?? "Unknown scenario failure");
  return failures.join("\n\n") || "Scenario report status is fail";
}

async function dryRunScenarios(options: RunOptions, unit: MigrationUnit, targetDir: string) {
  const tsx = path.join(options.harnessRoot, "node_modules", ".bin", "tsx");
  for (const scenario of unitScenarioNames(unit)) {
    const args = [
      "src/run.ts",
      "--agent",
      "amp",
      "--language",
      "effect",
      "--scenario",
      scenario,
      "--dry-run",
    ];
    await runRequiredCommand(tsx, args, options.harnessRoot, {
      env: { GOLEM_PATH: options.repoRoot, GOLEM_TARGET_DIR: targetDir },
    });
  }
}

function finishAttempt(
  attempt: MigrationAttempt,
  outcome: "passed" | "failed" | "blocked",
  error?: string,
) {
  attempt.finishedAt = timestamp();
  attempt.outcome = outcome;
  attempt.error = error;
}

async function commitUnit(
  options: RunOptions,
  unit: MigrationUnit,
  manifest: MigrationManifest,
): Promise<void> {
  if (!options.commitEach) return;
  const manifestRelativePath = path.relative(options.repoRoot, options.manifestPath);
  const paths = unitAllowedPaths(unit, manifestRelativePath);
  try {
    await runRequiredCommand("git", ["add", "--", ...paths], options.repoRoot);
    await runRequiredCommand(
      "git",
      ["commit", "-m", `Add Effect skill for ${unit.id}`],
      options.repoRoot,
      { echo: true },
    );
  } catch (error) {
    throw new ControllerError(
      `Verified ${unit.id}, but its local commit failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  validateManifestInventory(manifest);
}

async function verifyUnit(
  options: RunOptions,
  manifest: MigrationManifest,
  unit: MigrationUnit,
  attempt: MigrationAttempt,
): Promise<string | undefined> {
  await assertScopedChanges(options.repoRoot, unit, options.manifestPath);
  await validateUnitStatic(options.repoRoot, unit);
  const targetDir = await buildGolem(options.repoRoot, manifest.policy.buildProfile);
  await dryRunScenarios(options, unit, targetDir);

  for (const scenario of unit.verifyScenarios) {
    const { reportPath, report } = await runScenario(options, unit, attempt, scenario, targetDir);
    attempt.reports.push(reportPath);
    const failure = reportFailure(report, unit.targetSkill);
    if (failure) return `${scenario}:\n${failure}`;
  }
  return undefined;
}

async function runUnit(
  options: RunOptions,
  manifest: MigrationManifest,
  unit: MigrationUnit,
  attemptLimit = manifest.policy.maxAttempts,
): Promise<boolean> {
  let previousAttempt = unit.attempts[unit.attempts.length - 1];
  let workerThread = previousAttempt?.thread;

  await assertScopedChanges(options.repoRoot, unit, options.manifestPath);

  while (unit.attempts.length < attemptLimit) {
    const attempt: MigrationAttempt = {
      number: unit.attempts.length + 1,
      startedAt: timestamp(),
      outcome: "editing",
      reports: [],
    };
    unit.attempts.push(attempt);
    unit.state = options.verifyOnly ? "verifying" : "editing";
    await saveManifest(options.manifestPath, manifest);

    try {
      if (!options.verifyOnly) {
        const manifestBeforeWorker = await fs.readFile(options.manifestPath, "utf8");
        const prompt = await buildWorkerPrompt(options.repoRoot, manifest, unit, previousAttempt);
        const amp = await runAmpWorker(
          options.repoRoot,
          manifest.policy.workerMode,
          prompt,
          previousAttempt?.error ? workerThread : undefined,
        );
        attempt.thread = amp.thread ?? workerThread;
        workerThread = attempt.thread;
        const manifestAfterWorker = await fs.readFile(options.manifestPath, "utf8");
        if (manifestAfterWorker !== manifestBeforeWorker) {
          throw new ScopeViolationError(
            "Amp worker modified the controller-owned migration manifest; stopping without reverting it",
          );
        }
        await assertScopedChanges(options.repoRoot, unit, options.manifestPath);
        if (amp.exitCode !== 0) {
          throw new Error(amp.result ?? amp.stderr ?? "Amp worker failed");
        }
      }

      unit.state = "verifying";
      await saveManifest(options.manifestPath, manifest);
      const failure = await verifyUnit(options, manifest, unit, attempt);
      if (failure) throw new Error(failure);

      finishAttempt(attempt, "passed");
      unit.state = "passed";
      unit.evidence = {
        verifiedAt: timestamp(),
        effectGolemRef: manifest.effectGolem.ref,
        thread: attempt.thread,
        reports: [...attempt.reports],
      };
      await saveManifest(options.manifestPath, manifest);
      await commitUnit(options, unit, manifest);
      console.log(`Passed: ${unit.id}`);
      return true;
    } catch (error) {
      const failure = error instanceof Error ? error.message : String(error);
      if (error instanceof ScopeViolationError) {
        console.error(`Stopped: ${unit.id}\n${failure}`);
        throw error;
      }
      if (error instanceof ControllerError) {
        console.error(`Stopped: ${unit.id}\n${failure}`);
        throw error;
      }
      const blocked = unit.attempts.length >= attemptLimit;
      finishAttempt(attempt, blocked ? "blocked" : "failed", failure);
      previousAttempt = attempt;
      unit.state = blocked ? "blocked" : "editing";
      await saveManifest(options.manifestPath, manifest);
      console.error(`${blocked ? "Blocked" : "Retrying"}: ${unit.id}\n${failure}`);
      if (options.verifyOnly || blocked) return false;
    }
  }
  return false;
}

function printUsage(): void {
  console.log(`Effect skill migration controller

Usage:
  npm run migrate:effect -- [options]

Options:
  --manifest <path>    Manifest path (default: golem-skills/effect-migration.yaml)
  --unit <id>          Process a specific unit
  --all                Continue through all ready pending units
  --commit-each        Commit every passed unit locally (required with --all)
  --verify-only        Verify existing edits without launching an Amp worker
  --dry-run            Validate and print the selected queue without changing files
  --retry-blocked      Give the selected blocked unit a fresh bounded attempt budget
  --continue-on-block  Continue with independent units after a blocked unit
  -h, --help           Show this help

Run the controller from a dedicated, initially clean Git worktree. By default it processes one
ready unit and leaves the verified diff for review. Use --commit-each for resumable unattended
runs; the controller never pushes. GOLEM_EFFECT_GOLEM_PATH may point to an absolute local SDK
checkout; otherwise workers inspect the pinned repository ref from the manifest. A failed live
scenario resumes the migration thread with its report and generated workspace until the attempt
budget is exhausted.
`);
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      manifest: { type: "string" },
      unit: { type: "string" },
      all: { type: "boolean", default: false },
      "commit-each": { type: "boolean", default: false },
      "verify-only": { type: "boolean", default: false },
      "dry-run": { type: "boolean", default: false },
      "retry-blocked": { type: "boolean", default: false },
      "continue-on-block": { type: "boolean", default: false },
      help: { type: "boolean", short: "h", default: false },
    },
  });
  if (values.help) {
    printUsage();
    return;
  }
  if (values.all && !values["commit-each"] && !values["dry-run"]) {
    throw new Error("--all requires --commit-each so every completed unit is resumable");
  }
  if (values["retry-blocked"] && !values.unit) {
    throw new Error("--retry-blocked requires --unit <id>");
  }

  const thisDir = path.dirname(fileURLToPath(import.meta.url));
  const harnessRoot = path.resolve(thisDir, "..");
  const repoRoot = path.resolve(harnessRoot, "../../..");
  const manifestPath = path.resolve(
    repoRoot,
    values.manifest ?? "golem-skills/effect-migration.yaml",
  );
  const manifest = await loadManifest(manifestPath);
  await validateManifestFiles(repoRoot, manifest);

  if (values["dry-run"]) {
    const units = values.unit
      ? [selectNextUnit(manifest, values.unit)!]
      : manifest.units.filter(({ state }) => state !== "passed");
    for (const unit of units) {
      console.log(
        `${unit.state.padEnd(9)} ${unit.id} -> ${unit.targetSkill} [${unit.verifyScenarios.join(", ")}]`,
      );
    }
    return;
  }

  const options: RunOptions = {
    repoRoot,
    harnessRoot,
    manifestPath,
    commitEach: values["commit-each"],
    verifyOnly: values["verify-only"],
  };
  let processed = 0;
  while (true) {
    const unit = selectNextUnit(manifest, values.unit);
    if (!unit || (unit.state === "passed" && !options.verifyOnly)) break;
    const retryingBlocked =
      values["retry-blocked"] &&
      (unit.state === "blocked" || unit.attempts.length >= manifest.policy.maxAttempts);
    if (values["retry-blocked"] && !retryingBlocked) {
      throw new Error(`${unit.id} is not blocked or out of attempts`);
    }
    const attemptLimit = retryingBlocked
      ? unit.attempts.length + manifest.policy.maxAttempts
      : manifest.policy.maxAttempts;
    if (retryingBlocked) {
      console.log(
        `Retrying blocked unit ${unit.id} with ${manifest.policy.maxAttempts} additional attempts.`,
      );
    }
    const passed = await runUnit(options, manifest, unit, attemptLimit);
    processed++;
    const continueAfterBlocked = values["continue-on-block"] || !manifest.policy.stopOnBlocked;
    if (!passed && !continueAfterBlocked) break;
    if (!values.all || values.unit) break;
  }
  if (processed === 0) console.log("No ready Effect skill migration units.");
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : undefined;
if (invokedPath === fileURLToPath(import.meta.url)) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? (error.stack ?? error.message) : String(error));
    process.exitCode = 1;
  });
}
