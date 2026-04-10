import * as fs from "node:fs/promises";
import { spawn } from "node:child_process";
import * as path from "node:path";
import * as yaml from "yaml";
import { z } from "zod";
import { AgentDriver } from "./driver/base.js";
import { SkillWatcher } from "./watcher.js";
import { evaluate, ExpectSchema, type AssertionContext } from "./assertions.js";
import {
  classifyFailure,
  type FailureClassification,
} from "./failure-classification.js";
import { findGolemAppDir } from "./workspace.js";
import * as log from "./log.js";

export const DEFAULT_STEP_TIMEOUT_SECONDS = 300;

// --- Language-conditional resolution ---

const SUPPORTED_LANG_KEYS = new Set(["ts", "rust"]);

/**
 * Checks if a value is a language-keyed map (e.g., { ts: "...", rust: "..." }).
 * Returns true only if the value is a plain object whose keys are all known language codes.
 */
function isLanguageMap(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  return keys.length > 0 && keys.every((k) => SUPPORTED_LANG_KEYS.has(k));
}

/**
 * Resolves a field that can be either a plain value or a { ts: T, rust: T } map.
 * If it's a language map and a language is provided, returns the matching entry.
 * If it's a plain value (string, array, non-language object), returns it as-is.
 */
function resolveByLanguage<T>(
  value: T | Record<string, T> | undefined,
  language: string | undefined,
): T | undefined {
  if (value === undefined || value === null) return undefined;
  if (isLanguageMap(value) && language) {
    return (value as Record<string, T>)[language];
  }
  return value as T;
}

function tryParseJson<T>(value: string): T | undefined {
  try {
    return JSON.parse(value) as T;
  } catch {
    return undefined;
  }
}

function parseJsonCommandOutput<T>(output: string): T | undefined {
  const trimmed = output.trim();
  if (!trimmed) return undefined;

  const direct = tryParseJson<T>(trimmed);
  if (direct !== undefined) {
    return direct;
  }

  const lines = trimmed
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);

  for (let i = lines.length - 1; i >= 0; i--) {
    const parsed = tryParseJson<T>(lines[i]);
    if (parsed !== undefined) {
      return parsed;
    }
  }

  return undefined;
}

function extractInvokeJsonResult(output: string): unknown {
  const parsed = parseJsonCommandOutput<Record<string, unknown>>(output);
  if (!parsed || typeof parsed !== "object") {
    return parsed;
  }

  const resultJson = parsed.result_json;
  if (!resultJson || typeof resultJson !== "object") {
    return parsed;
  }

  return "value" in resultJson
    ? (resultJson as Record<string, unknown>).value
    : undefined;
}

// --- Schemas ---

const RetrySchema = z.object({
  attempts: z.number().int().min(1),
  delay: z.number().min(0),
});

const HttpSchema = z.object({
  url: z.string(),
  method: z.enum(["GET", "POST", "PUT", "DELETE", "PATCH"]).default("GET"),
  body: z.string().optional(),
  headers: z.record(z.string()).optional(),
});

const InvokeSchema = z.object({
  agent: z.string(),
  method: z.string(),
  args: z.string().optional(),
});

const ShellSchema = z.object({
  command: z.string(),
  args: z.array(z.string()).optional(),
  cwd: z.string().optional(),
});

const TriggerSchema = z.object({
  agent: z.string(),
  method: z.string(),
  args: z.string().optional(),
});

const CreateAgentSchema = z.object({
  name: z.string(),
  env: z.record(z.string()).optional(),
  config: z.record(z.string()).optional(),
});

const DeleteAgentSchema = z.object({
  name: z.string(),
});

const StepConditionSchema = z.object({
  agent: z.string().optional(),
  language: z.string().optional(),
  os: z.string().optional(),
});

const ACTION_FIELDS = [
  "prompt",
  "invoke",
  "invoke_json",
  "shell",
  "trigger",
  "create_agent",
  "delete_agent",
  "sleep",
  "http",
] as const;

// Language-conditional: accepts either T or { ts: T, rust: T, ... }
function langConditional<T extends z.ZodType>(schema: T) {
  return z.union([schema, z.record(z.string(), schema)]);
}

const VerifySchema = z.object({
  build: z.boolean().optional(),
  deploy: z.boolean().optional(),
  expectedFiles: langConditional(z.array(z.string())).optional(),
});

const StepSpecSchema = z
  .object({
    id: z.string().optional(),
    prompt: langConditional(z.string()).optional(),
    expectedSkills: langConditional(z.array(z.string())).optional(),
    allowedExtraSkills: langConditional(z.array(z.string())).optional(),
    strictSkillMatch: z.boolean().optional(),
    timeout: z.number().optional(),
    continue_session: z.boolean().optional(),
    verify: langConditional(VerifySchema).optional(),
    invoke: InvokeSchema.optional(),
    invoke_json: InvokeSchema.optional(),
    expect: ExpectSchema.optional(),
    sleep: z.number().optional(),
    shell: ShellSchema.optional(),
    trigger: TriggerSchema.optional(),
    create_agent: CreateAgentSchema.optional(),
    delete_agent: DeleteAgentSchema.optional(),
    http: HttpSchema.optional(),
    retry: RetrySchema.optional(),
    only_if: StepConditionSchema.optional(),
    skip_if: StepConditionSchema.optional(),
  })
  .refine(
    (step) => {
      const presentActions = ACTION_FIELDS.filter((f) => step[f] !== undefined);
      return presentActions.length === 1;
    },
    {
      message: `Step must have exactly one action field (${ACTION_FIELDS.join(", ")})`,
    },
  );

const SettingsSchema = z
  .object({
    timeout_per_subprompt: z.number().optional(),
    golem_server: z
      .object({
        router_port: z.number().optional(),
        custom_request_port: z.number().optional(),
      })
      .optional(),
    cleanup: z.boolean().optional(),
  })
  .optional();
const PrerequisitesSchema = z
  .object({
    env: z.record(z.string()).optional(),
  })
  .optional();

const ScenarioSpecSchema = z.object({
  name: z.string({ required_error: 'Scenario must have a "name" field' }),
  settings: SettingsSchema,
  prerequisites: PrerequisitesSchema,
  skip_if: StepConditionSchema.optional(),
  steps: z.array(StepSpecSchema).min(1, "Scenario must have at least one step"),
});

type LangConditional<T> = T | Record<string, T>;

interface StepCommon {
  id?: string;
  expectedSkills?: LangConditional<string[]>;
  allowedExtraSkills?: LangConditional<string[]>;
  strictSkillMatch?: boolean;
  timeout?: number;
  continue_session?: boolean;
  verify?: LangConditional<{
    build?: boolean;
    deploy?: boolean;
    expectedFiles?: LangConditional<string[]>;
  }>;
  expect?: z.infer<typeof ExpectSchema>;
  only_if?: StepCondition;
  skip_if?: StepCondition;
  retry?: { attempts: number; delay: number };
}

type InvokeSpec = { agent: string; method: string; args?: string };
type ShellSpec = { command: string; args?: string[]; cwd?: string };
type TriggerSpec = { agent: string; method: string; args?: string };
type CreateAgentSpec = {
  name: string;
  env?: Record<string, string>;
  config?: Record<string, string>;
};
type DeleteAgentSpec = { name: string };
type HttpSpec = {
  url: string;
  method?: "GET" | "POST" | "PUT" | "DELETE" | "PATCH";
  body?: string;
  headers?: Record<string, string>;
};

type RawStepSpec = z.infer<typeof StepSpecSchema>;

type PromptStep = StepCommon & { tag: "prompt"; prompt: LangConditional<string> };
type InvokeStep = StepCommon & { tag: "invoke"; invoke: InvokeSpec };
type InvokeJsonStep = StepCommon & { tag: "invoke_json"; invoke_json: InvokeSpec };
type ShellStep = StepCommon & { tag: "shell"; shell: ShellSpec };
type TriggerStep = StepCommon & { tag: "trigger"; trigger: TriggerSpec };
type CreateAgentStep = StepCommon & { tag: "create_agent"; create_agent: CreateAgentSpec };
type DeleteAgentStep = StepCommon & { tag: "delete_agent"; delete_agent: DeleteAgentSpec };
type SleepStep = StepCommon & { tag: "sleep"; sleep: number };
type HttpStep = StepCommon & { tag: "http"; http: HttpSpec };

export type StepSpec =
  | PromptStep
  | InvokeStep
  | InvokeJsonStep
  | ShellStep
  | TriggerStep
  | CreateAgentStep
  | DeleteAgentStep
  | SleepStep
  | HttpStep;

export interface ScenarioSpec {
  name: string;
  settings?: {
    timeout_per_subprompt?: number;
    golem_server?: {
      router_port?: number;
      custom_request_port?: number;
    };
    cleanup?: boolean;
  };
  prerequisites?: {
    env?: Record<string, string>;
  };
  skip_if?: StepCondition;
  steps: StepSpec[];
}

export function parseStep(raw: RawStepSpec): StepSpec {
  const tag = ACTION_FIELDS.find((f) => raw[f] !== undefined);
  if (!tag) throw new Error("Step has no action field");

  const common: StepCommon = {
    ...(raw.id !== undefined && { id: raw.id }),
    ...(raw.expectedSkills !== undefined && { expectedSkills: raw.expectedSkills }),
    ...(raw.allowedExtraSkills !== undefined && { allowedExtraSkills: raw.allowedExtraSkills }),
    ...(raw.strictSkillMatch !== undefined && { strictSkillMatch: raw.strictSkillMatch }),
    ...(raw.timeout !== undefined && { timeout: raw.timeout }),
    ...(raw.continue_session !== undefined && { continue_session: raw.continue_session }),
    ...(raw.verify !== undefined && { verify: raw.verify }),
    ...(raw.expect !== undefined && { expect: raw.expect }),
    ...(raw.only_if !== undefined && { only_if: raw.only_if }),
    ...(raw.skip_if !== undefined && { skip_if: raw.skip_if }),
    ...(raw.retry !== undefined && { retry: raw.retry }),
  };

  switch (tag) {
    case "prompt": return { ...common, tag, prompt: raw.prompt! };
    case "invoke": return { ...common, tag, invoke: raw.invoke! };
    case "invoke_json": return { ...common, tag, invoke_json: raw.invoke_json! };
    case "shell": return { ...common, tag, shell: raw.shell! };
    case "trigger": return { ...common, tag, trigger: raw.trigger! };
    case "create_agent": return { ...common, tag, create_agent: raw.create_agent! };
    case "delete_agent": return { ...common, tag, delete_agent: raw.delete_agent! };
    case "sleep": return { ...common, tag, sleep: raw.sleep! };
    case "http": return { ...common, tag, http: raw.http! };
  }
}

export class ScenarioLoader {
  static async load(filePath: string): Promise<ScenarioSpec> {
    const content = await fs.readFile(filePath, "utf8");
    const raw = yaml.parse(content);
    const result = ScenarioSpecSchema.safeParse(raw);
    if (!result.success) {
      const issues = result.error.issues
        .map((i) => `  - ${i.path.join(".")}: ${i.message}`)
        .join("\n");
      throw new Error(`Invalid scenario file "${filePath}":\n${issues}`);
    }
    const data = result.data;
    const steps = data.steps.map(parseStep);
    return { ...data, steps } as ScenarioSpec;
  }
}

export interface StepAttemptResult {
  attemptNumber: number;
  success: boolean;
  durationSeconds: number;
  error?: string;
  activatedSkills: string[];
}

export interface StepResult {
  step: StepSpec;
  success: boolean;
  durationSeconds: number;
  expectedSkills: string[];
  activatedSkills: string[];
  error?: string;
  attempts?: StepAttemptResult[];
  classification?: FailureClassification;
}

export interface ScenarioRunResult {
  status: "pass" | "fail";
  durationSeconds: number;
  stepResults: StepResult[];
  artifactPaths: string[];
  workspace: string;
}

interface LocalCommandResult {
  success: boolean;
  stdout: string;
  stderr: string;
  output: string;
  exitCode: number | null;
}

export interface ScenarioExecutorOptions {
  globalTimeoutSeconds?: number;
  agent?: string;
  language?: string;
  abortSignal?: AbortSignal;
  resumeFromStepId?: string;
}

// --- Template variable substitution ---

export function substituteVariables(
  text: string,
  variables: Record<string, string>,
): string {
  return text.replace(/\{\{(\w+)\}\}/g, (match, name: string) => {
    return variables[name] ?? match;
  });
}

// --- Conditional step execution ---

export interface StepCondition {
  agent?: string;
  language?: string;
  os?: string;
}

function normalizePlatform(platform: string): string {
  if (platform === "darwin") return "macos";
  if (platform === "win32") return "windows";
  return platform;
}

export function shouldRunStep(
  step: Pick<StepCommon, "only_if" | "skip_if">,
  context: { agent?: string; language?: string; os: string },
): boolean {
  const normalizedOs = normalizePlatform(context.os);

  if (step.only_if) {
    const cond = step.only_if;
    if (cond.agent && cond.agent !== context.agent) return false;
    if (cond.language && cond.language !== context.language) return false;
    if (cond.os && cond.os !== normalizedOs) return false;
  }

  if (step.skip_if) {
    const cond = step.skip_if;
    if (cond.agent && cond.agent === context.agent) return false;
    if (cond.language && cond.language === context.language) return false;
    if (cond.os && cond.os === normalizedOs) return false;
  }

  return true;
}

export class ScenarioExecutor {
  private driver: AgentDriver;
  private watcher: SkillWatcher;
  private workspace: string;
  private skillsDir: string;
  private options: ScenarioExecutorOptions;

  constructor(
    driver: AgentDriver,
    watcher: SkillWatcher,
    workspace: string,
    skillsDir: string,
    options?: ScenarioExecutorOptions,
  ) {
    this.driver = driver;
    this.watcher = watcher;
    this.workspace = workspace;
    this.skillsDir = skillsDir;
    this.options = options ?? {};
  }

  private buildVariables(scenarioName: string): Record<string, string> {
    const vars: Record<string, string> = {
      workspace: this.workspace,
      scenario: scenarioName,
    };
    if (this.options.agent) vars["agent"] = this.options.agent;
    if (this.options.language) vars["language"] = this.options.language;
    return vars;
  }

  private substituteStepVariables(
    step: StepSpec,
    variables: Record<string, string>,
  ): StepSpec {
    const sub = (s: string | undefined) =>
      s ? substituteVariables(s, variables) : s;
    const subArr = (arr: string[] | undefined) =>
      arr?.map((s) => substituteVariables(s, variables));
    const subLangStr = (v: LangConditional<string>): LangConditional<string> =>
      typeof v === "string"
        ? substituteVariables(v, variables)
        : Object.fromEntries(Object.entries(v).map(([k, s]) => [k, substituteVariables(s, variables)]));

    switch (step.tag) {
      case "prompt":
        return { ...step, prompt: subLangStr(step.prompt) };
      case "invoke":
        return { ...step, invoke: {
          agent: substituteVariables(step.invoke.agent, variables),
          method: substituteVariables(step.invoke.method, variables),
          args: sub(step.invoke.args),
        }};
      case "invoke_json":
        return { ...step, invoke_json: {
          agent: substituteVariables(step.invoke_json.agent, variables),
          method: substituteVariables(step.invoke_json.method, variables),
          args: sub(step.invoke_json.args),
        }};
      case "shell":
        return { ...step, shell: {
          command: substituteVariables(step.shell.command, variables),
          args: subArr(step.shell.args),
          cwd: sub(step.shell.cwd),
        }};
      case "trigger":
        return { ...step, trigger: {
          agent: substituteVariables(step.trigger.agent, variables),
          method: substituteVariables(step.trigger.method, variables),
          args: sub(step.trigger.args),
        }};
      case "create_agent":
        return { ...step, create_agent: {
          ...step.create_agent,
          name: substituteVariables(step.create_agent.name, variables),
        }};
      case "delete_agent":
        return { ...step, delete_agent: {
          ...step.delete_agent,
          name: substituteVariables(step.delete_agent.name, variables),
        }};
      case "http":
        return { ...step, http: {
          ...step.http,
          url: substituteVariables(step.http.url, variables),
          body: sub(step.http.body),
          headers: step.http.headers
            ? Object.fromEntries(
              Object.entries(step.http.headers).map(([k, v]) => [
                k,
                substituteVariables(v, variables),
              ]),
            )
            : step.http.headers,
        }};
      case "sleep":
        return { ...step };
    }
  }

  private resolveLanguageFields(step: StepSpec): StepSpec {
    const lang = this.options.language;
    const resolvedVerify = resolveByLanguage(step.verify, lang);
    const resolved = {
      ...step,
      expectedSkills: resolveByLanguage(step.expectedSkills, lang),
      allowedExtraSkills: resolveByLanguage(step.allowedExtraSkills, lang),
      verify: resolvedVerify
        ? { ...resolvedVerify, expectedFiles: resolveByLanguage(resolvedVerify.expectedFiles, lang) }
        : undefined,
    };
    if (step.tag === "prompt") {
      return { ...resolved, tag: "prompt", prompt: resolveByLanguage(step.prompt, lang)! } as StepSpec;
    }
    return resolved as StepSpec;
  }

  async execute(spec: ScenarioSpec): Promise<ScenarioRunResult> {
    // Scenario-level skip
    if (spec.skip_if) {
      const ctx = {
        agent: this.options.agent,
        language: this.options.language,
        os: process.platform,
      };
      // Reuse shouldRunStep with a fake step that has only skip_if
      if (!shouldRunStep({ skip_if: spec.skip_if }, ctx)) {
        log.scenarioSkip(spec.name);
        return {
          status: "pass",
          durationSeconds: 0,
          stepResults: [],
          artifactPaths: [],
          workspace: this.workspace,
        };
      }
    }

    const results: StepResult[] = [];
    const savedEnv: Record<string, string | undefined> = {};
    // Validate resumeFromStepId if set
    if (this.options.resumeFromStepId) {
      const found = spec.steps.some(
        (s) => s.id === this.options.resumeFromStepId,
      );
      if (!found) {
        throw new Error(
          `Resume step "${this.options.resumeFromStepId}" not found in scenario "${spec.name}"`,
        );
      }
    }

    // Set prerequisites env vars
    if (spec.prerequisites?.env) {
      for (const [key, value] of Object.entries(spec.prerequisites.env)) {
        savedEnv[key] = process.env[key];
        process.env[key] = value;
      }
    }

    // Setup workspace (each run gets a unique ID so no cleanup needed)
    await fs.mkdir(this.workspace, { recursive: true });
    await this.driver.setup(this.workspace, this.skillsDir);
    // Watch all agent skill directories — each agent reads skills from its own location
    const agentSkillsDirs = [
      ".claude/skills",
      ".agents/skills",
    ];
    for (const rel of agentSkillsDirs) {
      this.watcher.addWatchDir(path.join(this.workspace, rel));
    }
    await this.verifyGolemConnectivity(spec);
    await this.watcher.start();

    // Build extra env for commands from settings
    const commandEnv = this.buildCommandEnv(spec);
    const variables = this.buildVariables(spec.name);
    const conditionContext = {
      agent: this.options.agent,
      language: this.options.language,
      os: process.platform,
    };

    const startTime = Date.now();
    let isFirstPrompt = true;
    let resumeReached = !this.options.resumeFromStepId;
    try {
      for (const originalStep of spec.steps) {
        // Check abort signal
        if (this.options.abortSignal?.aborted) break;

        // Substitute template variables and resolve language-conditional fields
        const step = this.resolveLanguageFields(
          this.substituteStepVariables(originalStep, variables),
        );

        // Resume-from: skip steps before the target
        if (!resumeReached) {
          if (step.id === this.options.resumeFromStepId) {
            resumeReached = true;
          } else {
            log.stepSkip(step.id ?? "(unnamed)", "before resume point");
            results.push({
              step: originalStep,
              success: true,
              durationSeconds: 0,
              expectedSkills: (step.expectedSkills as string[] | undefined) ?? [],
              activatedSkills: [],
            });
            continue;
          }
        }

        // Conditional execution
        if (!shouldRunStep(step, conditionContext)) {
          log.stepSkip(step.id ?? "(unnamed)", "condition not met");
          results.push({
            step: originalStep,
            success: true,
            durationSeconds: 0,
            expectedSkills: (step.expectedSkills as string[] | undefined) ?? [],
            activatedSkills: [],
          });
          continue;
        }

        // Retry logic
        const maxAttempts = step.retry?.attempts ?? 1;
        const retryDelay = step.retry?.delay ?? 0;
        const attempts: StepAttemptResult[] = [];
        let finalResult:
          | {
            success: boolean;
            errors: string[];
            activatedSkills: string[];
            isFirstPrompt: boolean;
          }
          | undefined;

        for (let attempt = 1; attempt <= maxAttempts; attempt++) {
          if (attempt > 1) {
            log.stepRetry(step.id ?? "(unnamed)", attempt, maxAttempts, retryDelay);
            await new Promise((resolve) =>
              setTimeout(resolve, retryDelay * 1000),
            );
          }

          const attemptStart = Date.now();
          const bodyResult = await this.executeStepBody(
            step,
            spec,
            commandEnv,
            isFirstPrompt,
          );
          const attemptDuration = (Date.now() - attemptStart) / 1000;
          isFirstPrompt = bodyResult.isFirstPrompt;

          attempts.push({
            attemptNumber: attempt,
            success: bodyResult.success,
            durationSeconds: attemptDuration,
            error:
              bodyResult.errors.length > 0
                ? bodyResult.errors.join("\n")
                : undefined,
            activatedSkills: bodyResult.activatedSkills,
          });

          finalResult = bodyResult;
          if (bodyResult.success) break;
        }

        const totalDuration = attempts.reduce(
          (sum, a) => sum + a.durationSeconds,
          0,
        );
        const errorStr =
          finalResult!.errors.length > 0
            ? finalResult!.errors.join("\n")
            : undefined;
        const classification =
          errorStr && !finalResult!.success
            ? classifyFailure(errorStr)
            : undefined;

        results.push({
          step: originalStep,
          success: finalResult!.success,
          durationSeconds: totalDuration,
          expectedSkills: (step.expectedSkills as string[] | undefined) ?? [],
          activatedSkills: finalResult!.activatedSkills,
          error: errorStr,
          attempts: maxAttempts > 1 ? attempts : undefined,
          classification,
        });

        if (!finalResult!.success) break; // Stop on failure
      }
    } finally {
      // Restore env vars
      for (const [key, value] of Object.entries(savedEnv)) {
        if (value === undefined) {
          delete process.env[key];
        } else {
          process.env[key] = value;
        }
      }

      await this.watcher.stop();
      await this.driver.teardown();
    }

    return {
      status: results.every((result) => result.success) ? "pass" : "fail",
      durationSeconds: (Date.now() - startTime) / 1000,
      stepResults: results,
      artifactPaths: [this.workspace],
      workspace: this.workspace,
    };
  }

  private async executeStepBody(
    step: StepSpec,
    spec: ScenarioSpec,
    commandEnv: Record<string, string>,
    isFirstPrompt: boolean,
  ): Promise<{
    success: boolean;
    errors: string[];
    activatedSkills: string[];
    isFirstPrompt: boolean;
  }> {
    const errors: string[] = [];
    let success = true;
    const stepTimeoutSeconds =
      step.timeout ??
      spec.settings?.timeout_per_subprompt ??
      this.options.globalTimeoutSeconds ??
      DEFAULT_STEP_TIMEOUT_SECONDS;
    const stepBaseline = this.watcher.markBaseline();
    await this.watcher.snapshotAtimes();
    const stepLabel = step.id ?? "(unnamed)";
    log.stepStart(stepLabel, stepTimeoutSeconds);

    const fail = (msg: string) => {
      success = false;
      errors.push(msg);
    };

    // Dispatch action
    switch (step.tag) {
      case "sleep":
        await this.executeSleep(stepLabel, step.sleep);
        break;
      case "create_agent":
        await this.executeCreateAgent(stepLabel, step.create_agent, stepTimeoutSeconds, commandEnv, fail);
        break;
      case "delete_agent":
        await this.executeDeleteAgent(stepLabel, step.delete_agent, stepTimeoutSeconds, commandEnv, fail);
        break;
      case "shell":
        await this.executeShell(stepLabel, step.shell, step.expect, stepTimeoutSeconds, commandEnv, fail);
        break;
      case "trigger":
        await this.executeTrigger(stepLabel, step.trigger, stepTimeoutSeconds, commandEnv);
        break;
      case "prompt":
        isFirstPrompt = await this.executePrompt(stepLabel, step.prompt as string, step.continue_session, isFirstPrompt, stepTimeoutSeconds, fail);
        break;
      case "invoke":
        await this.executeInvoke(stepLabel, step.invoke, step.expect, stepTimeoutSeconds, commandEnv, fail);
        break;
      case "invoke_json":
        await this.executeInvokeJson(stepLabel, step.invoke_json, step.expect, stepTimeoutSeconds, commandEnv, fail);
        break;
      case "http":
        await this.executeHttp(stepLabel, step.http, step.expect, stepTimeoutSeconds, fail);
        break;
    }

    // Verify skills activation
    const activatedSkills = await this.verifySkillActivation(stepLabel, step, stepBaseline, fail);

    // Build/deploy/expectedFiles verification
    if (step.verify?.build || step.verify?.deploy || step.verify?.expectedFiles) {
      await this.executeVerification(stepLabel, step.verify as { build?: boolean; deploy?: boolean; expectedFiles?: string[] }, commandEnv, success, fail);
      success = errors.length === 0;
    }

    return { success, errors, activatedSkills, isFirstPrompt };
  }

  // --- Action handlers ---

  private async executeSleep(stepLabel: string, seconds: number): Promise<void> {
    log.stepAction(stepLabel, `sleeping for ${seconds}s`);
    await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
  }

  private async executeCreateAgent(
    stepLabel: string,
    spec: CreateAgentSpec,
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `creating agent "${spec.name}"`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["agent", "new", spec.name];
    if (spec.env) {
      for (const [k, v] of Object.entries(spec.env)) {
        args.push("-e", `${k}=${v}`);
      }
    }
    if (spec.config) {
      for (const [k, v] of Object.entries(spec.config)) {
        args.push("-c", `${k}=${v}`);
      }
    }
    const result = await this.runLocalCommand(
      "golem", args, timeout, projectDir, commandEnv,
    );
    if (!result.success) fail(`CREATE_AGENT_FAILED: ${result.output}`);
  }

  private async executeDeleteAgent(
    stepLabel: string,
    spec: DeleteAgentSpec,
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `deleting agent "${spec.name}"`);
    const projectDir = await this.findGolemProjectDir();
    const result = await this.runLocalCommand(
      "golem", ["agent", "delete", spec.name], timeout, projectDir, commandEnv,
    );
    if (!result.success) fail(`DELETE_AGENT_FAILED: ${result.output}`);
  }

  private async executeShell(
    stepLabel: string,
    shell: ShellSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `running shell command "${shell.command}"`);
    const shellCwd = shell.cwd
      ? path.resolve(this.workspace, shell.cwd)
      : this.workspace;
    const result = await this.runLocalCommand(
      shell.command, shell.args ?? [], timeout, shellCwd, commandEnv,
    );

    if (expect) {
      this.evaluateAssertions({ stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode }, expect, fail);
    } else if (!result.success) {
      fail(`SHELL_FAILED: ${result.output}`);
    }
  }

  private async executeTrigger(
    stepLabel: string,
    trigger: TriggerSpec,
    timeout: number,
    commandEnv: Record<string, string>,
  ): Promise<void> {
    log.stepAction(stepLabel, `triggering ${trigger.agent}.${trigger.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["agent", "invoke", trigger.agent, trigger.method, "--trigger"];
    if (trigger.args) args.push(trigger.args);
    this.runLocalCommand("golem", args, timeout, projectDir, commandEnv)
      .catch(() => { /* fire and forget */ });
  }

  private async executePrompt(
    stepLabel: string,
    prompt: string,
    continueSession: boolean | undefined,
    isFirstPrompt: boolean,
    timeout: number,
    fail: (msg: string) => void,
  ): Promise<boolean> {
    const useContinueSession = continueSession !== false && !isFirstPrompt;
    if (useContinueSession) {
      log.stepAction(stepLabel, "sending followup prompt");
      const result = await this.driver.sendFollowup(prompt, timeout);
      if (!result.success) fail(`Agent failed: ${result.output}`);
    } else {
      log.stepAction(stepLabel, "sending prompt");
      const result = await this.driver.sendPrompt(prompt, timeout);
      if (!result.success) fail(`Agent failed: ${result.output}`);
    }
    return false; // isFirstPrompt = false after any prompt
  }

  private async executeInvoke(
    stepLabel: string,
    invoke: InvokeSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `invoking ${invoke.agent}.${invoke.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["agent", "invoke", invoke.agent, invoke.method];
    if (invoke.args) args.push(invoke.args);
    const result = await this.runLocalCommand(
      "golem", args, timeout, projectDir, commandEnv,
    );

    if (expect) {
      let resultJson: unknown;
      try { resultJson = JSON.parse(result.stdout); } catch { /* not JSON */ }
      this.evaluateAssertions(
        { stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode, resultJson },
        expect, fail,
      );
    } else if (!result.success) {
      fail(`INVOKE_FAILED: ${result.output}`);
    }
  }

  private async executeInvokeJson(
    stepLabel: string,
    invoke: InvokeSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `invoking (json) ${invoke.agent}.${invoke.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["--format", "json", "agent", "invoke", invoke.agent, invoke.method];
    if (invoke.args) args.push(invoke.args);
    const result = await this.runLocalCommand(
      "golem", args, timeout, projectDir, commandEnv,
    );

    const resultJson = extractInvokeJsonResult(result.stdout);

    if (expect) {
      this.evaluateAssertions(
        { stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode, resultJson },
        expect, fail,
      );
    } else if (!result.success) {
      fail(`INVOKE_JSON_FAILED: ${result.output}`);
    }
  }

  private async executeHttp(
    stepLabel: string,
    http: HttpSpec,
    expect: StepCommon["expect"],
    timeoutSeconds: number,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `HTTP ${http.method ?? "GET"} ${http.url}`);
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeoutSeconds * 1000);
    const onParentAbort = () => controller.abort();
    this.options.abortSignal?.addEventListener("abort", onParentAbort);
    try {
      const response = await fetch(http.url, {
        method: http.method ?? "GET",
        body: http.body,
        headers: http.headers,
        signal: controller.signal,
      });
      const body = await response.text();

      if (expect) {
        this.evaluateAssertions(
          { stdout: body, stderr: "", exitCode: response.ok ? 0 : 1, body, status: response.status },
          expect, fail,
        );
      } else if (!response.ok) {
        fail(`HTTP_FAILED: ${response.status} ${body.slice(0, 500)}`);
      }
    } catch (err) {
      if (err instanceof Error && err.name === "AbortError") {
        fail(`HTTP_FAILED: request timed out after ${timeoutSeconds}s`);
      } else {
        fail(`HTTP_FAILED: ${err instanceof Error ? err.message : String(err)}`);
      }
    } finally {
      clearTimeout(timeoutId);
      this.options.abortSignal?.removeEventListener("abort", onParentAbort);
    }
  }

  // --- Shared helpers ---

  private evaluateAssertions(
    ctx: AssertionContext,
    expect: z.infer<typeof ExpectSchema>,
    fail: (msg: string) => void,
  ): void {
    for (const ar of evaluate(ctx, expect)) {
      if (!ar.passed) fail(`ASSERTION_FAILED (${ar.assertion}): ${ar.message}`);
    }
  }

  private async verifySkillActivation(
    stepLabel: string,
    step: StepSpec,
    baseline: ReturnType<SkillWatcher["markBaseline"]>,
    fail: (msg: string) => void,
  ): Promise<string[]> {
    const watcherEvents = this.watcher.getActivatedEventsSince(baseline);
    const atimeResults = await this.watcher.getSkillsWithChangedAtime();
    for (const evt of watcherEvents) {
      log.stepSkillDetected(stepLabel, "fswatch", evt.skillName, evt.path);
    }
    for (const res of atimeResults) {
      log.stepSkillDetected(stepLabel, "atime", res.skillName, res.path);
    }
    const activatedSkills = Array.from(
      new Set([
        ...watcherEvents.map((e) => e.skillName),
        ...atimeResults.map((r) => r.skillName),
      ]),
    );
    log.stepActivatedSkills(stepLabel, activatedSkills);
    const error = this.assertSkillActivation(step, activatedSkills);
    if (error) fail(error);
    return activatedSkills;
  }

  private async executeVerification(
    stepLabel: string,
    verify: { build?: boolean; deploy?: boolean; expectedFiles?: string[] },
    commandEnv: Record<string, string>,
    currentSuccess: boolean,
    fail: (msg: string) => void,
  ): Promise<void> {
    const projectDir = await this.findGolemProjectDir();

    if (verify.expectedFiles) {
      const expectedCount = verify.expectedFiles.length;
      const fileLabel = expectedCount === 1 ? "file" : "files";
      let missingCount = 0;

      log.stepAction(stepLabel, `verifying ${expectedCount} expected ${fileLabel}`);
      for (const relPath of verify.expectedFiles) {
        const fullPath = path.join(this.workspace, relPath);
        try {
          await fs.access(fullPath);
          log.stepAction(stepLabel, `expected file exists: ${relPath}`);
        } catch {
          missingCount += 1;
          fail(`EXPECTED_FILE_MISSING: ${relPath}`);
        }
      }

      if (missingCount === 0) {
        log.stepAction(stepLabel, `expected ${fileLabel} verified`);
      } else {
        log.stepAction(stepLabel, `expected ${fileLabel} verification failed (${missingCount} missing)`);
      }
    }

    if (verify.build) {
      log.stepAction(stepLabel, `running golem build in ${projectDir}`);
      const result = await this.runLocalCommand("golem", ["build"], 600, projectDir, commandEnv);
      if (!result.success) fail(`BUILD_FAILED: ${result.output}`);
    }

    if (verify.deploy) {
      // Deploy implies build — run build first if not already run
      if (!verify.build) {
        const golemTempDir = path.join(projectDir, "golem-temp");
        await fs.rm(golemTempDir, { recursive: true, force: true });
        log.stepAction(stepLabel, `running implicit golem build before deploy in ${projectDir}`);
        const buildResult = await this.runLocalCommand("golem", ["build"], 600, projectDir, commandEnv);
        if (!buildResult.success) {
          fail(`BUILD_FAILED: ${buildResult.output}`);
          return;
        }
      }

      if (currentSuccess) {
        log.stepAction(stepLabel, `running golem deploy in ${projectDir}`);
        const deployResult = await this.runLocalCommand("golem", ["deploy", "--yes"], 600, projectDir, commandEnv);
        if (!deployResult.success) fail(`DEPLOY_FAILED: ${deployResult.output}`);
      }
    }
  }

  private assertSkillActivation(
    step: StepSpec,
    activatedSkills: string[],
  ): string | undefined {
    const expectedSkills = (step.expectedSkills as string[] | undefined) ?? [];
    if (expectedSkills.length === 0) {
      return undefined;
    }

    const expectedSet = new Set(expectedSkills);
    const activatedSet = new Set(activatedSkills);

    for (const expected of expectedSet) {
      if (!activatedSet.has(expected)) {
        return `SKILL_NOT_ACTIVATED: expected "${expected}" but activated [${activatedSkills.join(", ")}]`;
      }
    }

    if (step.strictSkillMatch) {
      const extras = activatedSkills.filter((skill) => !expectedSet.has(skill));
      if (extras.length > 0) {
        return `SKILL_MISMATCH: strict match enabled; unexpected skills [${extras.join(", ")}]`;
      }
      return undefined;
    }

    const allowedExtras = new Set((step.allowedExtraSkills as string[] | undefined) ?? []);
    const unexpectedExtras = activatedSkills.filter(
      (skill) => !expectedSet.has(skill) && !allowedExtras.has(skill),
    );
    if (unexpectedExtras.length > 0) {
      return `SKILL_MISMATCH: unexpected extra skills [${unexpectedExtras.join(", ")}]`;
    }

    return undefined;
  }

  private buildCommandEnv(spec: ScenarioSpec): Record<string, string> {
    const env: Record<string, string> = {};
    if (spec.settings?.golem_server?.router_port) {
      env["GOLEM_ROUTER_PORT"] = String(spec.settings.golem_server.router_port);
    }
    if (spec.settings?.golem_server?.custom_request_port) {
      env["GOLEM_CUSTOM_REQUEST_PORT"] = String(
        spec.settings.golem_server.custom_request_port,
      );
    }
    if (spec.prerequisites?.env) {
      Object.assign(env, spec.prerequisites.env);
    }
    return env;
  }

  private async verifyGolemConnectivity(spec?: ScenarioSpec): Promise<void> {
    const routerPort = spec?.settings?.golem_server?.router_port ?? 9881;

    const profileCheck = await this.runLocalCommand(
      "golem",
      ["profile", "get", "--profile", "local"],
      30,
      this.workspace,
    );
    if (!profileCheck.success) {
      throw new Error(
        `Failed to verify local Golem profile: ${profileCheck.output}`,
      );
    }

    const serverCheck = await this.runLocalCommand(
      "curl",
      ["-fsS", `http://localhost:${routerPort}/healthcheck`],
      30,
      this.workspace,
    );
    if (!serverCheck.success) {
      throw new Error(
        `Failed to connect to local Golem server on localhost:${routerPort}: ${serverCheck.output}`,
      );
    }
  }

  private async findGolemProjectDir(): Promise<string> {
    return findGolemAppDir(this.workspace);
  }

  private async runLocalCommand(
    command: string,
    args: string[],
    timeoutSeconds: number,
    cwd: string,
    extraEnv?: Record<string, string>,
  ): Promise<LocalCommandResult> {
    const controller = new AbortController();
    const { signal } = controller;

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        signal,
        env: { ...process.env, ...extraEnv },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let stdout = "";
      let stderr = "";
      child.stdout?.on("data", (data) => (stdout += data.toString()));
      child.stderr?.on("data", (data) => (stderr += data.toString()));

      const timeoutId = setTimeout(() => {
        controller.abort();
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
        resolve({
          success: exitCode === 0,
          stdout,
          stderr,
          output: stdout + stderr,
          exitCode,
        });
      });

      child.on("error", (error) => {
        clearTimeout(timeoutId);
        resolve({
          success: false,
          stdout,
          stderr,
          output: stdout + stderr + error.message,
          exitCode: null,
        });
      });
    });
  }
}
