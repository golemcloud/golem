import * as fs from "node:fs/promises";
import { spawn } from "node:child_process";
import * as path from "node:path";
import * as yaml from "yaml";
import { z } from "zod";
import {
  AgentDriver,
  killProcessTree,
  type DriverTimeoutOptions,
  type UsageStats,
} from "./driver/base.js";
import { SkillWatcher } from "./watcher.js";
import { evaluate, ExpectSchema, type AssertionContext } from "./assertions.js";
import { classifyFailure, type FailureClassification } from "./failure-classification.js";
import { findGolemAppDir } from "./workspace.js";
import { startPrerequisiteServices, type PrerequisiteServiceName } from "./services.js";
import * as log from "./log.js";

export const DEFAULT_STEP_TIMEOUT_SECONDS = 1800;
export const DEFAULT_IDLE_TIMEOUT_SECONDS = 300;
const WATCHER_SNAPSHOT_SETTLE_MS = 25;

// --- Language-conditional resolution ---

const SUPPORTED_LANG_KEYS = new Set(["ts", "rust", "scala", "moonbit"]);

/**
 * Checks if a value is a language-keyed map (e.g., { ts: "...", rust: "...", scala: "..." }).
 * Returns true only if the value is a plain object whose keys are all known language codes.
 */
function isLanguageMap(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  return keys.length > 0 && keys.every((k) => SUPPORTED_LANG_KEYS.has(k));
}

/**
 * Resolves a field that can be either a plain value or a language-keyed map.
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

  return "value" in resultJson ? (resultJson as Record<string, unknown>).value : undefined;
}

// --- Schemas ---

const RetrySchema = z.object({
  attempts: z.number().int().min(1),
  delay: z.number().min(0),
});

const HttpSchema = z.object({
  url: z.string(),
  method: z.enum(["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]).default("GET"),
  body: langConditional(z.string()).optional(),
  headers: z.record(z.string()).optional(),
});

const GetAgentTypeSchema = z.object({
  name: z.string(),
});

const ListAgentTypesSchema = z.object({});

const CheckFileSchema = z.object({
  path: z.string(),
});

const McpCallSchema = z.object({
  url: z.string(),
  method: z.string(),
  params: z.record(z.unknown()).optional(),
});

const InvokeSchema = z.object({
  agent: z.string(),
  method: langConditional(z.string()),
  args: langConditional(z.string()).optional(),
});

const ShellSchema = z.object({
  command: z.string(),
  args: langConditional(z.array(z.string())).optional(),
  cwd: z.string().optional(),
});

const TriggerSchema = z.object({
  agent: z.string(),
  method: langConditional(z.string()),
  args: langConditional(z.string()).optional(),
});

const CreateAgentSchema = z.object({
  name: z.string(),
  env: z.record(z.string()).optional(),
  config: z.record(z.union([z.string(), z.number(), z.boolean()])).optional(),
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
  "create_project",
  "sleep",
  "http",
  "get_agent_type",
  "list_agent_types",
  "check_file",
  "mcp_call",
] as const;

// Language-conditional: accepts either T or { ts: T, rust: T, scala: T, ... }
function langConditional<T extends z.ZodType>(schema: T) {
  return z.union([schema, z.record(z.string(), schema)]);
}

const CreateProjectSchema = z.object({
  name: z.string(),
  presets: langConditional(z.array(z.string())).optional(),
});

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
    continueSession: z.boolean().optional(),
    verify: langConditional(VerifySchema).optional(),
    invoke: InvokeSchema.optional(),
    invoke_json: InvokeSchema.optional(),
    expect: langConditional(ExpectSchema).optional(),
    sleep: z.number().optional(),
    shell: ShellSchema.optional(),
    trigger: TriggerSchema.optional(),
    create_agent: CreateAgentSchema.optional(),
    delete_agent: DeleteAgentSchema.optional(),
    create_project: CreateProjectSchema.optional(),
    http: HttpSchema.optional(),
    get_agent_type: GetAgentTypeSchema.optional(),
    list_agent_types: ListAgentTypesSchema.optional(),
    check_file: CheckFileSchema.optional(),
    mcp_call: McpCallSchema.optional(),
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
  )
  .superRefine((step, ctx) => {
    if (
      step.expectedSkills === undefined &&
      (step.allowedExtraSkills !== undefined || step.strictSkillMatch)
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["expectedSkills"],
        message: "expectedSkills is required when allowedExtraSkills or strictSkillMatch is set",
      });
    }
  });

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
const ServiceNameSchema = z.enum(["postgres", "mysql", "ignite", "openai-mock"]);
const PrerequisitesSchema = z
  .object({
    env: z.record(z.string()).optional(),
    services: z.array(ServiceNameSchema).optional(),
  })
  .optional();

const ScenarioSpecSchema = z.object({
  name: z.string({ required_error: 'Scenario must have a "name" field' }),
  languageAgnostic: z.boolean().optional(),
  settings: SettingsSchema,
  prerequisites: PrerequisitesSchema,
  skip_if: StepConditionSchema.optional(),
  steps: z.array(StepSpecSchema).min(1, "Scenario must have at least one step"),
  finally: z.array(StepSpecSchema).optional(),
});

type LangConditional<T> = T | Record<string, T>;

interface StepCommon {
  id?: string;
  expectedSkills?: LangConditional<string[]>;
  allowedExtraSkills?: LangConditional<string[]>;
  strictSkillMatch?: boolean;
  timeout?: number;
  continueSession?: boolean;
  verify?: LangConditional<{
    build?: boolean;
    deploy?: boolean;
    expectedFiles?: LangConditional<string[]>;
  }>;
  expect?: LangConditional<z.infer<typeof ExpectSchema>>;
  only_if?: StepCondition;
  skip_if?: StepCondition;
  retry?: { attempts: number; delay: number };
}

type InvokeSpec = {
  agent: string;
  method: LangConditional<string>;
  args?: LangConditional<string>;
};
type ShellSpec = { command: string; args?: LangConditional<string[]>; cwd?: string };
type ResolvedShellSpec = { command: string; args?: string[]; cwd?: string };
type TriggerSpec = {
  agent: string;
  method: LangConditional<string>;
  args?: LangConditional<string>;
};
type ResolvedInvokeSpec = { agent: string; method: string; args?: string };
type ResolvedTriggerSpec = { agent: string; method: string; args?: string };
type CreateAgentSpec = {
  name: string;
  env?: Record<string, string>;
  config?: Record<string, string | number | boolean>;
};
type DeleteAgentSpec = { name: string };
type CreateProjectSpec = { name: string; presets?: LangConditional<string[]> };
type HttpSpec = {
  url: string;
  method?: "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "OPTIONS";
  body?: LangConditional<string>;
  headers?: Record<string, string>;
};
type GetAgentTypeSpec = { name: string };
type ListAgentTypesSpec = Record<string, never>;

type RawStepSpec = z.infer<typeof StepSpecSchema>;

type PromptStep = StepCommon & { tag: "prompt"; prompt: LangConditional<string> };
type InvokeStep = StepCommon & { tag: "invoke"; invoke: InvokeSpec };
type InvokeJsonStep = StepCommon & { tag: "invoke_json"; invoke_json: InvokeSpec };
type ShellStep = StepCommon & { tag: "shell"; shell: ShellSpec };
type TriggerStep = StepCommon & { tag: "trigger"; trigger: TriggerSpec };
type CreateAgentStep = StepCommon & { tag: "create_agent"; create_agent: CreateAgentSpec };
type DeleteAgentStep = StepCommon & { tag: "delete_agent"; delete_agent: DeleteAgentSpec };
type CreateProjectStep = StepCommon & { tag: "create_project"; create_project: CreateProjectSpec };
type SleepStep = StepCommon & { tag: "sleep"; sleep: number };
type HttpStep = StepCommon & { tag: "http"; http: HttpSpec };
type GetAgentTypeStep = StepCommon & { tag: "get_agent_type"; get_agent_type: GetAgentTypeSpec };
type ListAgentTypesStep = StepCommon & {
  tag: "list_agent_types";
  list_agent_types: ListAgentTypesSpec;
};
type CheckFileSpec = { path: string };
type CheckFileStep = StepCommon & { tag: "check_file"; check_file: CheckFileSpec };
type McpCallSpec = { url: string; method: string; params?: Record<string, unknown> };
type McpCallStep = StepCommon & { tag: "mcp_call"; mcp_call: McpCallSpec };

export type StepSpec =
  | PromptStep
  | InvokeStep
  | InvokeJsonStep
  | ShellStep
  | TriggerStep
  | CreateAgentStep
  | DeleteAgentStep
  | CreateProjectStep
  | SleepStep
  | HttpStep
  | GetAgentTypeStep
  | ListAgentTypesStep
  | CheckFileStep
  | McpCallStep;

export interface ScenarioSpec {
  name: string;
  languageAgnostic?: boolean;
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
    services?: PrerequisiteServiceName[];
  };
  skip_if?: StepCondition;
  steps: StepSpec[];
  finally?: StepSpec[];
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
    ...(raw.continueSession !== undefined && { continueSession: raw.continueSession }),
    ...(raw.verify !== undefined && { verify: raw.verify }),
    ...(raw.expect !== undefined && { expect: raw.expect }),
    ...(raw.only_if !== undefined && { only_if: raw.only_if }),
    ...(raw.skip_if !== undefined && { skip_if: raw.skip_if }),
    ...(raw.retry !== undefined && { retry: raw.retry }),
  };

  switch (tag) {
    case "prompt":
      return { ...common, tag, prompt: raw.prompt! };
    case "invoke":
      return { ...common, tag, invoke: raw.invoke! };
    case "invoke_json":
      return { ...common, tag, invoke_json: raw.invoke_json! };
    case "shell":
      return { ...common, tag, shell: raw.shell! };
    case "trigger":
      return { ...common, tag, trigger: raw.trigger! };
    case "create_agent":
      return { ...common, tag, create_agent: raw.create_agent! };
    case "delete_agent":
      return { ...common, tag, delete_agent: raw.delete_agent! };
    case "create_project":
      return { ...common, tag, create_project: raw.create_project! };
    case "sleep":
      return { ...common, tag, sleep: raw.sleep! };
    case "http":
      return { ...common, tag, http: raw.http! };
    case "get_agent_type":
      return { ...common, tag, get_agent_type: raw.get_agent_type! };
    case "list_agent_types":
      return { ...common, tag, list_agent_types: raw.list_agent_types ?? {} };
    case "check_file":
      return { ...common, tag, check_file: raw.check_file! };
    case "mcp_call":
      return { ...common, tag, mcp_call: raw.mcp_call! };
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
    const finallySteps = data.finally?.map(parseStep);
    return { ...data, steps, ...(finallySteps && { finally: finallySteps }) } as ScenarioSpec;
  }
}

export interface StepAttemptResult {
  attemptNumber: number;
  success: boolean;
  durationSeconds: number;
  error?: string;
  activatedSkills: string[];
  timedOut?: boolean;
  timeoutKind?: "step" | "idle";
  usage?: UsageStats;
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
  timedOut?: boolean;
  timeoutKind?: "step" | "idle";
  usage?: UsageStats;
}

export interface ScenarioRunResult {
  status: "pass" | "fail";
  durationSeconds: number;
  stepResults: StepResult[];
  artifactPaths: string[];
  workspace: string;
  /** True when a step detected a credit-balance-too-low error from the LLM provider. */
  creditInsufficient?: boolean;
  usage?: UsageStats;
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
  idleTimeoutSeconds?: number;
  agent?: string;
  language?: string;
  abortSignal?: AbortSignal;
  resumeFromStepId?: string;
}

// --- Template variable substitution ---

export function substituteVariables(text: string, variables: Record<string, string>): string {
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
  private watcherStarted = false;
  private currentSkillSessionBaseline: number | undefined;
  private workspace: string;
  private bootstrapSkillSourceDirs: string[];
  private options: ScenarioExecutorOptions;
  private routerPort: number = 9881;

  constructor(
    driver: AgentDriver,
    watcher: SkillWatcher,
    workspace: string,
    bootstrapSkillSourceDirs: string[],
    options?: ScenarioExecutorOptions,
  ) {
    this.driver = driver;
    this.watcher = watcher;
    this.workspace = workspace;
    this.bootstrapSkillSourceDirs = bootstrapSkillSourceDirs;
    this.options = options ?? {};
  }

  private buildVariables(
    scenarioName: string,
    extraVariables?: Record<string, string>,
  ): Record<string, string> {
    const vars: Record<string, string> = {
      workspace: this.workspace,
      scenario: scenarioName,
    };
    if (this.options.agent) vars["agent"] = this.options.agent;
    if (this.options.language) vars["language"] = this.options.language;
    if (extraVariables) Object.assign(vars, extraVariables);
    return vars;
  }

  private substituteStepVariables(step: StepSpec, variables: Record<string, string>): StepSpec {
    const sub = (s: string | undefined) => (s ? substituteVariables(s, variables) : s);
    const _subArr = (arr: string[] | undefined) =>
      arr?.map((s) => substituteVariables(s, variables));
    const subLangStr = (v: LangConditional<string>): LangConditional<string> =>
      typeof v === "string"
        ? substituteVariables(v, variables)
        : Object.fromEntries(
            Object.entries(v).map(([k, s]) => [k, substituteVariables(s, variables)]),
          );
    const subLangStrOpt = (
      v: LangConditional<string> | undefined,
    ): LangConditional<string> | undefined => (v === undefined ? undefined : subLangStr(v));
    const subLangArr = (v: LangConditional<string[]>): LangConditional<string[]> =>
      Array.isArray(v)
        ? v.map((s) => substituteVariables(s, variables))
        : Object.fromEntries(
            Object.entries(v).map(([k, arr]) => [
              k,
              arr.map((s: string) => substituteVariables(s, variables)),
            ]),
          );
    const subLangArrOpt = (
      v: LangConditional<string[]> | undefined,
    ): LangConditional<string[]> | undefined => (v === undefined ? undefined : subLangArr(v));

    switch (step.tag) {
      case "prompt":
        return { ...step, prompt: subLangStr(step.prompt) };
      case "invoke":
        return {
          ...step,
          invoke: {
            agent: substituteVariables(step.invoke.agent, variables),
            method: subLangStr(step.invoke.method),
            args: subLangStrOpt(step.invoke.args),
          },
        };
      case "invoke_json":
        return {
          ...step,
          invoke_json: {
            agent: substituteVariables(step.invoke_json.agent, variables),
            method: subLangStr(step.invoke_json.method),
            args: subLangStrOpt(step.invoke_json.args),
          },
        };
      case "shell":
        return {
          ...step,
          shell: {
            command: substituteVariables(step.shell.command, variables),
            args: subLangArrOpt(step.shell.args),
            cwd: sub(step.shell.cwd),
          },
        };
      case "trigger":
        return {
          ...step,
          trigger: {
            agent: substituteVariables(step.trigger.agent, variables),
            method: subLangStr(step.trigger.method),
            args: subLangStrOpt(step.trigger.args),
          },
        };
      case "create_agent":
        return {
          ...step,
          create_agent: {
            ...step.create_agent,
            name: substituteVariables(step.create_agent.name, variables),
          },
        };
      case "delete_agent":
        return {
          ...step,
          delete_agent: {
            ...step.delete_agent,
            name: substituteVariables(step.delete_agent.name, variables),
          },
        };
      case "http":
        return {
          ...step,
          http: {
            ...step.http,
            url: substituteVariables(step.http.url, variables),
            body: subLangStrOpt(step.http.body),
            headers: step.http.headers
              ? Object.fromEntries(
                  Object.entries(step.http.headers).map(([k, v]) => [
                    k,
                    substituteVariables(v, variables),
                  ]),
                )
              : step.http.headers,
          },
        };
      case "create_project":
        return { ...step };
      case "sleep":
        return { ...step };
      case "get_agent_type":
        return {
          ...step,
          get_agent_type: {
            ...step.get_agent_type,
            name: substituteVariables(step.get_agent_type.name, variables),
          },
        };
      case "list_agent_types":
        return { ...step };
      case "check_file":
        return {
          ...step,
          check_file: {
            ...step.check_file,
            path: substituteVariables(step.check_file.path, variables),
          },
        };
      case "mcp_call":
        return {
          ...step,
          mcp_call: {
            ...step.mcp_call,
            url: substituteVariables(step.mcp_call.url, variables),
          },
        };
    }
  }

  private resolveLanguageFields(step: StepSpec): StepSpec {
    const lang = this.options.language;
    const resolvedVerify = resolveByLanguage(step.verify, lang);
    const resolved = {
      ...step,
      expectedSkills: resolveByLanguage(step.expectedSkills, lang),
      allowedExtraSkills: resolveByLanguage(step.allowedExtraSkills, lang),
      expect: resolveByLanguage(step.expect, lang),
      verify: resolvedVerify
        ? {
            ...resolvedVerify,
            expectedFiles: resolveByLanguage(resolvedVerify.expectedFiles, lang),
          }
        : undefined,
    };
    if (step.tag === "prompt") {
      return {
        ...resolved,
        tag: "prompt",
        prompt: resolveByLanguage(step.prompt, lang)!,
      } as StepSpec;
    }
    if (step.tag === "invoke") {
      return {
        ...resolved,
        tag: "invoke",
        invoke: {
          ...step.invoke,
          method: resolveByLanguage(step.invoke.method, lang)!,
          args: resolveByLanguage(step.invoke.args, lang),
        },
      } as StepSpec;
    }
    if (step.tag === "invoke_json") {
      return {
        ...resolved,
        tag: "invoke_json",
        invoke_json: {
          ...step.invoke_json,
          method: resolveByLanguage(step.invoke_json.method, lang)!,
          args: resolveByLanguage(step.invoke_json.args, lang),
        },
      } as StepSpec;
    }
    if (step.tag === "trigger") {
      return {
        ...resolved,
        tag: "trigger",
        trigger: {
          ...step.trigger,
          method: resolveByLanguage(step.trigger.method, lang)!,
          args: resolveByLanguage(step.trigger.args, lang),
        },
      } as StepSpec;
    }
    if (step.tag === "shell") {
      return {
        ...resolved,
        tag: "shell",
        shell: {
          ...step.shell,
          args: resolveByLanguage(step.shell.args, lang),
        },
      } as StepSpec;
    }
    if (step.tag === "http") {
      return {
        ...resolved,
        tag: "http",
        http: {
          ...step.http,
          body: resolveByLanguage(step.http.body, lang),
        },
      } as StepSpec;
    }
    return resolved as StepSpec;
  }

  private ensureResolvedInvokeSpec(invoke: InvokeSpec): ResolvedInvokeSpec {
    if (typeof invoke.method !== "string") {
      throw new Error("Invoke method must resolve to a string for the current language");
    }
    if (invoke.args !== undefined && typeof invoke.args !== "string") {
      throw new Error("Invoke args must resolve to a string for the current language");
    }

    return {
      ...invoke,
      method: invoke.method,
      args: invoke.args as string | undefined,
    };
  }

  private ensureResolvedShellSpec(shell: ShellSpec): ResolvedShellSpec {
    if (shell.args !== undefined && !Array.isArray(shell.args)) {
      throw new Error("Shell args must resolve to a string array for the current language");
    }
    return {
      ...shell,
      args: shell.args as string[] | undefined,
    };
  }

  private ensureResolvedTriggerSpec(trigger: TriggerSpec): ResolvedTriggerSpec {
    if (typeof trigger.method !== "string") {
      throw new Error("Trigger method must resolve to a string for the current language");
    }
    if (trigger.args !== undefined && typeof trigger.args !== "string") {
      throw new Error("Trigger args must resolve to a string for the current language");
    }

    return {
      ...trigger,
      method: trigger.method,
      args: trigger.args as string | undefined,
    };
  }

  private startsNewPromptSession(step: StepSpec, isFirstPrompt: boolean): boolean {
    return step.tag === "prompt" && (isFirstPrompt || step.continueSession === false);
  }

  private driverTracksSkillsNatively(): boolean {
    return this.driver.getActivatedSkills() !== undefined;
  }

  private async ensureWatcherStarted(): Promise<void> {
    if (!this.watcherStarted) {
      await this.watcher.start();
      this.watcherStarted = true;
    }
  }

  private async beginSkillTrackingSession(): Promise<void> {
    this.driver.resetActivatedSkills();

    if (this.driverTracksSkillsNatively()) {
      this.currentSkillSessionBaseline = undefined;
      return;
    }

    await this.ensureWatcherStarted();
    await this.watcher.snapshotAtimes();
    await new Promise((resolve) => setTimeout(resolve, WATCHER_SNAPSHOT_SETTLE_MS));
    this.currentSkillSessionBaseline = this.watcher.markBaseline();
  }

  private async ensureSkillTrackingReadyForStep(shouldTrackSkills: boolean): Promise<number> {
    if (this.driverTracksSkillsNatively()) {
      return 0;
    }

    if (this.currentSkillSessionBaseline !== undefined) {
      return this.currentSkillSessionBaseline;
    }

    if (!shouldTrackSkills) {
      return 0;
    }

    await this.ensureWatcherStarted();
    await this.watcher.snapshotAtimes();
    await new Promise((resolve) => setTimeout(resolve, WATCHER_SNAPSHOT_SETTLE_MS));
    this.currentSkillSessionBaseline = this.watcher.markBaseline();
    return this.currentSkillSessionBaseline;
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
    const startedServices = await startPrerequisiteServices(spec.prerequisites?.services);
    // Validate resumeFromStepId if set
    if (this.options.resumeFromStepId) {
      const found = spec.steps.some((s) => s.id === this.options.resumeFromStepId);
      if (!found) {
        throw new Error(
          `Resume step "${this.options.resumeFromStepId}" not found in scenario "${spec.name}"`,
        );
      }
    }

    // Set prerequisite service env vars first, then scenario-provided env vars so
    // the scenario can override defaults when needed.
    for (const [key, value] of Object.entries(startedServices.env)) {
      if (!(key in savedEnv)) {
        savedEnv[key] = process.env[key];
      }
      process.env[key] = value;
    }

    // Set prerequisites env vars
    if (spec.prerequisites?.env) {
      for (const [key, value] of Object.entries(spec.prerequisites.env)) {
        if (!(key in savedEnv)) {
          savedEnv[key] = process.env[key];
        }
        process.env[key] = value;
      }
    }

    const startTime = Date.now();
    let isFirstPrompt = true;
    let resumeReached = !this.options.resumeFromStepId;
    let creditInsufficient = false;
    // Build env and variables early so finalizers can use them even if setup fails
    const commandEnv = this.buildCommandEnv(spec, startedServices.env);
    this.routerPort = spec.settings?.golem_server?.router_port ?? 9881;
    const variables = this.buildVariables(spec.name, startedServices.variables);

    try {
      // Setup workspace (each run gets a unique ID so no cleanup needed)
      this.currentSkillSessionBaseline = undefined;
      await fs.mkdir(this.workspace, { recursive: true });
      await this.driver.setup(this.workspace, this.bootstrapSkillSourceDirs);
      await this.verifyGolemConnectivity(spec);
      const conditionContext = {
        agent: this.options.agent,
        language: this.options.language,
        os: process.platform,
      };

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
              timedOut?: boolean;
              timeoutKind?: "step" | "idle";
              creditInsufficient?: boolean;
              usage?: UsageStats;
            }
          | undefined;

        for (let attempt = 1; attempt <= maxAttempts; attempt++) {
          if (attempt > 1) {
            log.stepRetry(step.id ?? "(unnamed)", attempt, maxAttempts, retryDelay);
            await new Promise((resolve) => setTimeout(resolve, retryDelay * 1000));
          }

          const attemptStart = Date.now();
          const bodyResult = await this.executeStepBody(step, spec, commandEnv, isFirstPrompt);
          const attemptDuration = (Date.now() - attemptStart) / 1000;
          isFirstPrompt = bodyResult.isFirstPrompt;

          attempts.push({
            attemptNumber: attempt,
            success: bodyResult.success,
            durationSeconds: attemptDuration,
            error: bodyResult.errors.length > 0 ? bodyResult.errors.join("\n") : undefined,
            activatedSkills: bodyResult.activatedSkills,
            timedOut: bodyResult.timedOut,
            timeoutKind: bodyResult.timeoutKind,
            usage: bodyResult.usage,
          });

          finalResult = bodyResult;
          if (bodyResult.creditInsufficient) {
            creditInsufficient = true;
            break;
          }
          if (bodyResult.success) break;
        }

        const totalDuration = attempts.reduce((sum, a) => sum + a.durationSeconds, 0);
        const errorStr =
          finalResult!.errors.length > 0 ? finalResult!.errors.join("\n") : undefined;
        const classification =
          errorStr && !finalResult!.success ? classifyFailure(errorStr) : undefined;

        results.push({
          step: originalStep,
          success: finalResult!.success,
          durationSeconds: totalDuration,
          expectedSkills: (step.expectedSkills as string[] | undefined) ?? [],
          activatedSkills: finalResult!.activatedSkills,
          error: errorStr,
          attempts: maxAttempts > 1 ? attempts : undefined,
          classification,
          timedOut: finalResult!.timedOut,
          timeoutKind: finalResult!.timeoutKind,
          usage: finalResult!.usage,
        });

        if (!finalResult!.success) break; // Stop on failure
      }
    } finally {
      // Run finalizer steps (best-effort: all run even if some fail, abort signal is ignored)
      if (spec.finally?.length) {
        const savedAbortSignal = this.options.abortSignal;
        this.options.abortSignal = undefined;
        try {
          await this.executeFinalizers(spec, commandEnv, variables, results);
        } finally {
          this.options.abortSignal = savedAbortSignal;
        }
      }

      try {
        await startedServices.stopAll();
      } catch (err) {
        log.stepAction("finally", `failed to stop services: ${err}`);
      }

      // Restore env vars
      for (const [key, value] of Object.entries(savedEnv)) {
        if (value === undefined) {
          delete process.env[key];
        } else {
          process.env[key] = value;
        }
      }

      try {
        await this.watcher.stop();
      } catch (err) {
        log.stepAction("finally", `failed to stop watcher: ${err}`);
      }
      this.watcherStarted = false;

      try {
        await this.driver.teardown();
      } catch (err) {
        log.stepAction("finally", `failed to teardown driver: ${err}`);
      }
    }

    const aggregatedUsage = this.aggregateUsage(results);

    return {
      status: results.every((result) => result.success) ? "pass" : "fail",
      durationSeconds: (Date.now() - startTime) / 1000,
      stepResults: results,
      artifactPaths: [this.workspace],
      workspace: this.workspace,
      creditInsufficient,
      usage: aggregatedUsage,
    };
  }

  private aggregateUsage(results: StepResult[]): UsageStats | undefined {
    let hasAny = false;
    let inputTokens = 0;
    let outputTokens = 0;
    let costUsd = 0;
    let numTurns = 0;
    for (const r of results) {
      if (r.usage) {
        hasAny = true;
        inputTokens += r.usage.inputTokens ?? 0;
        outputTokens += r.usage.outputTokens ?? 0;
        costUsd += r.usage.costUsd ?? 0;
        numTurns += r.usage.numTurns ?? 0;
      }
    }
    if (!hasAny) return undefined;
    return {
      ...(inputTokens > 0 && { inputTokens }),
      ...(outputTokens > 0 && { outputTokens }),
      ...(costUsd > 0 && { costUsd }),
      ...(numTurns > 0 && { numTurns }),
    };
  }

  private async executeFinalizers(
    spec: ScenarioSpec,
    commandEnv: Record<string, string>,
    variables: Record<string, string>,
    results: StepResult[],
  ): Promise<void> {
    log.stepAction("finally", "running finalizer steps");
    for (const originalStep of spec.finally!) {
      const stepLabel = `finally:${originalStep.id ?? "(unnamed)"}`;
      const stepStart = Date.now();
      try {
        const step = this.resolveLanguageFields(
          this.substituteStepVariables(originalStep, variables),
        );
        const bodyResult = await this.executeStepBody(step, spec, commandEnv, false);
        results.push({
          step: originalStep,
          success: bodyResult.success,
          durationSeconds: (Date.now() - stepStart) / 1000,
          expectedSkills: [],
          activatedSkills: bodyResult.activatedSkills,
          error: bodyResult.errors.length > 0 ? bodyResult.errors.join("\n") : undefined,
        });
        if (!bodyResult.success) {
          log.stepAction(stepLabel, `finalizer failed: ${bodyResult.errors.join(", ")}`);
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        log.stepAction(stepLabel, `finalizer threw: ${msg}`);
        results.push({
          step: originalStep,
          success: false,
          durationSeconds: (Date.now() - stepStart) / 1000,
          expectedSkills: [],
          activatedSkills: [],
          error: `FINALIZER_ERROR: ${msg}`,
        });
      }
    }
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
    timedOut?: boolean;
    timeoutKind?: "step" | "idle";
    creditInsufficient?: boolean;
    usage?: UsageStats;
  }> {
    const errors: string[] = [];
    let success = true;
    let stepTimedOut: boolean | undefined;
    let stepTimeoutKind: "step" | "idle" | undefined;
    let stepCreditInsufficient: boolean | undefined;
    let stepUsage: UsageStats | undefined;
    const stepTimeoutSeconds =
      step.timeout ??
      spec.settings?.timeout_per_subprompt ??
      this.options.globalTimeoutSeconds ??
      DEFAULT_STEP_TIMEOUT_SECONDS;
    const startsNewPromptSession = this.startsNewPromptSession(step, isFirstPrompt);
    if (startsNewPromptSession) {
      await this.beginSkillTrackingSession();
    }
    const shouldTrackSkills = this.needsSkillTracking(step);
    const driverTracksSkills = shouldTrackSkills && this.driverTracksSkillsNatively();
    const stepBaseline = await this.ensureSkillTrackingReadyForStep(shouldTrackSkills);
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
        await this.executeCreateAgent(
          stepLabel,
          step.create_agent,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "delete_agent":
        await this.executeDeleteAgent(
          stepLabel,
          step.delete_agent,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "create_project":
        await this.executeCreateProject(
          stepLabel,
          step.create_project,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "shell":
        await this.executeShell(
          stepLabel,
          this.ensureResolvedShellSpec(step.shell),
          step.expect,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "trigger":
        await this.executeTrigger(
          stepLabel,
          this.ensureResolvedTriggerSpec(step.trigger),
          stepTimeoutSeconds,
          commandEnv,
        );
        break;
      case "prompt": {
        const idleTimeout =
          this.options.idleTimeoutSeconds ??
          this.driver.getDefaultIdleTimeoutSeconds() ??
          DEFAULT_IDLE_TIMEOUT_SECONDS;
        const promptResult = await this.executePrompt(
          stepLabel,
          step.prompt as string,
          step.continueSession,
          isFirstPrompt,
          stepTimeoutSeconds,
          idleTimeout,
          fail,
        );
        stepTimedOut = promptResult.timedOut;
        stepTimeoutKind = promptResult.timeoutKind;
        stepCreditInsufficient = promptResult.creditInsufficient;
        stepUsage = promptResult.usage;
        break;
      }
      case "invoke":
        await this.executeInvoke(
          stepLabel,
          this.ensureResolvedInvokeSpec(step.invoke),
          step.expect,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "invoke_json":
        await this.executeInvokeJson(
          stepLabel,
          this.ensureResolvedInvokeSpec(step.invoke_json),
          step.expect,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "http":
        await this.executeHttp(stepLabel, step.http, step.expect, stepTimeoutSeconds, fail);
        break;
      case "get_agent_type":
        await this.executeGetAgentType(
          stepLabel,
          step.get_agent_type,
          step.expect,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "list_agent_types":
        await this.executeListAgentTypes(
          stepLabel,
          step.expect,
          stepTimeoutSeconds,
          commandEnv,
          fail,
        );
        break;
      case "check_file":
        await this.executeCheckFile(stepLabel, step.check_file, step.expect, fail);
        break;
      case "mcp_call":
        await this.executeMcpCall(stepLabel, step.mcp_call, step.expect, stepTimeoutSeconds, fail);
        break;
    }

    // Verify skills activation
    const activatedSkills = shouldTrackSkills
      ? await this.verifySkillActivation(stepLabel, step, stepBaseline, driverTracksSkills, fail)
      : [];

    // Build/deploy/expectedFiles verification
    if (step.verify?.build || step.verify?.deploy || step.verify?.expectedFiles) {
      await this.executeVerification(
        stepLabel,
        step.verify as { build?: boolean; deploy?: boolean; expectedFiles?: string[] },
        commandEnv,
        success,
        fail,
      );
      success = errors.length === 0;
    }

    // After any step that may have created a project, update the agent driver's
    // working directory to the app directory so it finds AGENTS.md and
    // .agents/skills/ directly. This covers both create_project steps and prompt
    // steps where the agent creates the project itself.
    try {
      const appDir = await this.findGolemProjectDir();
      if (appDir !== this.workspace) {
        log.stepAction(stepLabel, `agent cwd → ${appDir}`);
        this.driver.setWorkingDirectory(appDir);
      }
    } catch {
      // No golem app found yet — that's fine
    }

    return {
      success,
      errors,
      activatedSkills,
      isFirstPrompt: step.tag === "prompt" && success ? false : isFirstPrompt,
      timedOut: stepTimedOut,
      timeoutKind: stepTimeoutKind,
      creditInsufficient: stepCreditInsufficient,
      usage: stepUsage,
    };
  }

  private needsSkillTracking(step: StepSpec): boolean {
    return Boolean(
      (step.expectedSkills as string[] | undefined)?.length ||
      (step.allowedExtraSkills as string[] | undefined)?.length ||
      step.strictSkillMatch,
    );
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
        args.push("-c", `${k}=${JSON.stringify(v)}`);
      }
    }
    const result = await this.runLocalCommand("golem", args, timeout, projectDir, commandEnv);
    log.cliOutput(stepLabel, "golem agent new", result.output);
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
      "golem",
      ["agent", "delete", spec.name],
      timeout,
      projectDir,
      commandEnv,
    );
    log.cliOutput(stepLabel, "golem agent delete", result.output);
    if (!result.success) fail(`DELETE_AGENT_FAILED: ${result.output}`);
  }

  private async executeCreateProject(
    stepLabel: string,
    spec: CreateProjectSpec,
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    const template = this.options.language;
    if (!template) {
      fail("CREATE_PROJECT_FAILED: language must be specified for create_project steps");
      return;
    }
    log.stepAction(stepLabel, `creating project "${spec.name}" with template "${template}"`);
    const args = ["new", spec.name, "--template", template, "--yes"];
    const presets = resolveByLanguage(spec.presets, this.options.language);
    if (presets) {
      for (const preset of presets) {
        args.push("--preset", preset);
      }
    }
    const result = await this.runLocalCommand("golem", args, timeout, this.workspace, commandEnv);
    log.cliOutput(stepLabel, "golem new", result.output);
    if (!result.success) fail(`CREATE_PROJECT_FAILED: ${result.output}`);
  }

  private async executeShell(
    stepLabel: string,
    shell: ResolvedShellSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    const fullCommand = [shell.command, ...(shell.args ?? [])].join(" ");
    log.stepAction(stepLabel, `running shell command: ${fullCommand}`);
    const shellCwd = shell.cwd ? path.resolve(this.workspace, shell.cwd) : this.workspace;
    const result = await this.runLocalCommand(
      shell.command,
      shell.args ?? [],
      timeout,
      shellCwd,
      commandEnv,
      (stream, data) => log.stepOutput(stepLabel, stream, data),
    );

    log.stepAction(stepLabel, `exit code: ${result.exitCode}`);

    if (expect) {
      this.evaluateAssertions(
        { stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode },
        expect,
        fail,
        stepLabel,
      );
    } else if (!result.success) {
      fail(`SHELL_FAILED: ${result.output}`);
    }
  }

  private async executeCheckFile(
    stepLabel: string,
    spec: CheckFileSpec,
    expect: StepCommon["expect"],
    fail: (msg: string) => void,
  ): Promise<void> {
    let baseDir: string;
    try {
      baseDir = await this.findGolemProjectDir();
    } catch {
      baseDir = this.workspace;
    }
    const resolvedPath = path.resolve(baseDir, spec.path);
    log.stepAction(stepLabel, `checking file: ${resolvedPath}`);

    let content: string;
    try {
      content = await fs.readFile(resolvedPath, "utf-8");
    } catch (err) {
      fail(`FILE_CHECK_FAILED: could not read file "${resolvedPath}": ${err}`);
      return;
    }

    log.stepAction(stepLabel, `file size: ${content.length} bytes`);

    if (expect) {
      this.evaluateAssertions(
        { stdout: content, stderr: "", exitCode: 0 },
        expect,
        fail,
        stepLabel,
      );
    }
  }

  private async executeTrigger(
    stepLabel: string,
    trigger: ResolvedTriggerSpec,
    timeout: number,
    commandEnv: Record<string, string>,
  ): Promise<void> {
    log.stepAction(stepLabel, `triggering ${trigger.agent}.${trigger.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["agent", "invoke", trigger.agent, trigger.method, "--trigger"];
    if (trigger.args) args.push(trigger.args);
    this.runLocalCommand("golem", args, timeout, projectDir, commandEnv).catch(() => {
      /* fire and forget */
    });
  }

  private async executePrompt(
    stepLabel: string,
    prompt: string,
    continueSession: boolean | undefined,
    isFirstPrompt: boolean,
    timeout: number,
    idleTimeout: number | undefined,
    fail: (msg: string) => void,
  ): Promise<{
    timedOut?: boolean;
    timeoutKind?: "step" | "idle";
    creditInsufficient?: boolean;
    usage?: UsageStats;
  }> {
    const opts: DriverTimeoutOptions = {
      stepTimeoutSeconds: timeout,
      idleTimeoutSeconds: idleTimeout,
    };
    const useContinueSession = continueSession !== false && !isFirstPrompt;
    if (useContinueSession) {
      log.stepPrompt(stepLabel, prompt, "followup");
      const result = await this.driver.sendFollowup(prompt, opts);
      if (!result.success) fail(`Agent failed: ${result.output}`);
      return {
        timedOut: result.timedOut,
        timeoutKind: result.timeoutKind,
        creditInsufficient: result.creditInsufficient,
        usage: result.usage,
      };
    } else {
      log.stepPrompt(stepLabel, prompt, "initial");
      const result = await this.driver.sendPrompt(prompt, opts);
      if (!result.success) fail(`Agent failed: ${result.output}`);
      return {
        timedOut: result.timedOut,
        timeoutKind: result.timeoutKind,
        creditInsufficient: result.creditInsufficient,
        usage: result.usage,
      };
    }
  }

  private async executeInvoke(
    stepLabel: string,
    invoke: ResolvedInvokeSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `invoking ${invoke.agent}.${invoke.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["agent", "invoke", invoke.agent, invoke.method];
    if (invoke.args) args.push(invoke.args);
    const result = await this.runLocalCommand("golem", args, timeout, projectDir, commandEnv);

    log.invokeResult(stepLabel, `${invoke.agent}.${invoke.method}`, result.stdout);
    if (result.stderr.trim()) {
      log.stepLine(stepLabel, `stderr: ${result.stderr.trim()}`);
    }

    if (expect) {
      let resultJson: unknown;
      try {
        resultJson = JSON.parse(result.stdout);
      } catch {
        /* not JSON */
      }
      this.evaluateAssertions(
        { stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode, resultJson },
        expect,
        fail,
        stepLabel,
      );
    } else if (!result.success) {
      fail(`INVOKE_FAILED: ${result.output}`);
    }
  }

  private async executeInvokeJson(
    stepLabel: string,
    invoke: ResolvedInvokeSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `invoking (json) ${invoke.agent}.${invoke.method}`);
    const projectDir = await this.findGolemProjectDir();
    const args = ["--format", "json", "agent", "invoke", invoke.agent, invoke.method];
    if (invoke.args) args.push(invoke.args);
    const result = await this.runLocalCommand("golem", args, timeout, projectDir, commandEnv);

    const resultJson = extractInvokeJsonResult(result.stdout);

    log.invokeResult(stepLabel, `${invoke.agent}.${invoke.method}`, result.stdout);
    if (result.stderr.trim()) {
      log.stepLine(stepLabel, `stderr: ${result.stderr.trim()}`);
    }
    log.stepLine(stepLabel, `extracted resultJson: ${JSON.stringify(resultJson)}`);

    if (expect) {
      this.evaluateAssertions(
        { stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode, resultJson },
        expect,
        fail,
        stepLabel,
      );
    } else if (!result.success) {
      fail(`INVOKE_JSON_FAILED: ${result.output}`);
    }
  }

  private async resolveDeploymentContext(
    stepLabel: string,
    timeout: number,
    commandEnv: Record<string, string>,
  ): Promise<{ environmentId: string; deploymentRevision: number } | undefined> {
    const projectDir = await this.findGolemProjectDir();
    log.stepAction(stepLabel, "resolving deployment context via golem environment list");
    const result = await this.runLocalCommand(
      "golem",
      ["--format", "json", "environment", "list"],
      timeout,
      projectDir,
      commandEnv,
    );
    if (!result.success) {
      return undefined;
    }

    const parsed = parseJsonCommandOutput<unknown[]>(result.stdout);
    if (!Array.isArray(parsed) || parsed.length === 0) {
      return undefined;
    }

    const env = parsed[0] as Record<string, unknown>;
    const envSummary = env["environment"] as Record<string, unknown> | undefined;
    if (!envSummary) {
      return undefined;
    }
    const environmentId = envSummary["id"] as string | undefined;
    const currentDeployment = envSummary["currentDeployment"] as
      | Record<string, unknown>
      | undefined;
    const deploymentRevision = currentDeployment?.["deploymentRevision"] as number | undefined;

    if (!environmentId || deploymentRevision === undefined) {
      return undefined;
    }

    log.stepAction(stepLabel, `resolved env=${environmentId} deployment=${deploymentRevision}`);
    return { environmentId, deploymentRevision };
  }

  private getRouterPort(): number {
    return this.routerPort ?? 9881;
  }

  private static readonly LOCAL_WELL_KNOWN_TOKEN = "5c832d93-ff85-4a8f-9803-513950fdfdb1";

  private golemApiHeaders(): Record<string, string> {
    return { Authorization: `Bearer ${ScenarioExecutor.LOCAL_WELL_KNOWN_TOKEN}` };
  }

  private async executeGetAgentType(
    stepLabel: string,
    spec: GetAgentTypeSpec,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    const ctx = await this.resolveDeploymentContext(stepLabel, timeout, commandEnv);
    if (!ctx) {
      fail("GET_AGENT_TYPE_FAILED: could not resolve deployment context");
      return;
    }

    const routerPort = this.getRouterPort();
    const url = `http://localhost:${routerPort}/v1/envs/${ctx.environmentId}/deployments/${ctx.deploymentRevision}/agent-types/${spec.name}`;
    log.stepAction(stepLabel, `GET ${url}`);

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), timeout * 1000);
      const onParentAbort = () => controller.abort();
      this.options.abortSignal?.addEventListener("abort", onParentAbort);

      try {
        const response = await fetch(url, {
          signal: controller.signal,
          headers: this.golemApiHeaders(),
        });
        const body = await response.text();
        log.httpResponse(stepLabel, response.status, body);

        if (expect) {
          this.evaluateAssertions(
            {
              stdout: body,
              stderr: "",
              exitCode: response.ok ? 0 : 1,
              body,
              status: response.status,
            },
            expect,
            fail,
            stepLabel,
          );
        } else if (!response.ok) {
          fail(`GET_AGENT_TYPE_FAILED: ${response.status} ${body.slice(0, 500)}`);
        }
      } finally {
        clearTimeout(timeoutId);
        this.options.abortSignal?.removeEventListener("abort", onParentAbort);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log.httpFailure(stepLabel, msg);
      fail(`GET_AGENT_TYPE_FAILED: ${msg}`);
    }
  }

  private async executeListAgentTypes(
    stepLabel: string,
    expect: StepCommon["expect"],
    timeout: number,
    commandEnv: Record<string, string>,
    fail: (msg: string) => void,
  ): Promise<void> {
    const ctx = await this.resolveDeploymentContext(stepLabel, timeout, commandEnv);
    if (!ctx) {
      fail("LIST_AGENT_TYPES_FAILED: could not resolve deployment context");
      return;
    }

    const routerPort = this.getRouterPort();
    const url = `http://localhost:${routerPort}/v1/envs/${ctx.environmentId}/deployments/${ctx.deploymentRevision}/agent-types`;
    log.stepAction(stepLabel, `GET ${url}`);

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), timeout * 1000);
      const onParentAbort = () => controller.abort();
      this.options.abortSignal?.addEventListener("abort", onParentAbort);

      try {
        const response = await fetch(url, {
          signal: controller.signal,
          headers: this.golemApiHeaders(),
        });
        const body = await response.text();
        log.httpResponse(stepLabel, response.status, body);

        if (expect) {
          this.evaluateAssertions(
            {
              stdout: body,
              stderr: "",
              exitCode: response.ok ? 0 : 1,
              body,
              status: response.status,
            },
            expect,
            fail,
            stepLabel,
          );
        } else if (!response.ok) {
          fail(`LIST_AGENT_TYPES_FAILED: ${response.status} ${body.slice(0, 500)}`);
        }
      } finally {
        clearTimeout(timeoutId);
        this.options.abortSignal?.removeEventListener("abort", onParentAbort);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log.httpFailure(stepLabel, msg);
      fail(`LIST_AGENT_TYPES_FAILED: ${msg}`);
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
        body: http.body as string | undefined,
        headers: http.headers,
        signal: controller.signal,
      });
      const body = await response.text();

      log.httpResponse(stepLabel, response.status, body);

      if (expect) {
        const headers: Record<string, string> = {};
        response.headers.forEach((value, key) => {
          headers[key.toLowerCase()] = value;
        });
        this.evaluateAssertions(
          {
            stdout: body,
            stderr: "",
            exitCode: response.ok ? 0 : 1,
            body,
            status: response.status,
            headers,
          },
          expect,
          fail,
          stepLabel,
        );
      } else if (!response.ok) {
        fail(`HTTP_FAILED: ${response.status} ${body.slice(0, 500)}`);
      }
    } catch (err) {
      if (err instanceof Error && err.name === "AbortError") {
        log.httpFailure(stepLabel, `request timed out after ${timeoutSeconds}s`);
        fail(`HTTP_FAILED: request timed out after ${timeoutSeconds}s`);
      } else {
        log.httpFailure(stepLabel, err instanceof Error ? err.message : String(err));
        fail(`HTTP_FAILED: ${err instanceof Error ? err.message : String(err)}`);
      }
    } finally {
      clearTimeout(timeoutId);
      this.options.abortSignal?.removeEventListener("abort", onParentAbort);
    }
  }

  private async executeMcpCall(
    stepLabel: string,
    spec: McpCallSpec,
    expect: StepCommon["expect"],
    timeoutSeconds: number,
    fail: (msg: string) => void,
  ): Promise<void> {
    log.stepAction(stepLabel, `MCP ${spec.method} → ${spec.url}`);

    const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
    const { StreamableHTTPClientTransport } =
      await import("@modelcontextprotocol/sdk/client/streamableHttp.js");
    const { CallToolResultSchema } = await import("@modelcontextprotocol/sdk/types.js");

    const client = new Client(
      { name: "golem-skill-harness", version: "1.0.0" },
      { capabilities: {} },
    );
    const transport = new StreamableHTTPClientTransport(new URL(spec.url));
    let connected = false;

    try {
      await Promise.race([
        client.connect(transport),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new Error("MCP connect timed out")), timeoutSeconds * 1000),
        ),
      ]);
      connected = true;

      let body: string;

      if (spec.method === "tools/list") {
        const result = await client.listTools();
        body = JSON.stringify(result);
      } else if (spec.method === "tools/call") {
        const params = spec.params ?? {};
        const result = await client.callTool(
          {
            name: params.name as string,
            arguments: (params.arguments as Record<string, unknown>) ?? {},
          },
          CallToolResultSchema,
        );
        body = JSON.stringify(result);
      } else if (spec.method === "resources/list") {
        const result = await client.listResources();
        body = JSON.stringify(result);
      } else if (spec.method === "prompts/list") {
        const result = await client.listPrompts();
        body = JSON.stringify(result);
      } else {
        log.mcpFailure(stepLabel, `unsupported MCP method: ${spec.method}`);
        fail(`MCP_CALL_FAILED: unsupported method "${spec.method}"`);
        return;
      }

      log.mcpResponse(stepLabel, spec.method, 200, body);

      if (expect) {
        this.evaluateAssertions(
          { stdout: body, stderr: "", exitCode: 0, body, status: 200, headers: {} },
          expect,
          fail,
          stepLabel,
        );
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      log.mcpFailure(stepLabel, message);
      fail(`MCP_CALL_FAILED: ${message}`);
    } finally {
      try {
        if (connected) {
          await client.close();
        }
      } catch {
        // ignore client close errors
      }
      try {
        await transport.terminateSession();
      } catch {
        // ignore terminateSession errors — server may not support it
      }
      try {
        await transport.close();
      } catch {
        // ignore transport close errors
      }
    }
  }

  // --- Shared helpers ---

  private evaluateAssertions(
    ctx: AssertionContext,
    expect: z.infer<typeof ExpectSchema>,
    fail: (msg: string) => void,
    stepLabel?: string,
  ): void {
    for (const ar of evaluate(ctx, expect)) {
      if (ar.passed) {
        if (stepLabel) {
          log.assertionPassed(stepLabel, ar.assertion, ar.message);
        }
      } else {
        if (stepLabel) {
          log.assertionFailed(stepLabel, ar.assertion, ar.message);
        }
        fail(`ASSERTION_FAILED (${ar.assertion}): ${ar.message}`);
      }
    }
  }

  private async verifySkillActivation(
    stepLabel: string,
    step: StepSpec,
    baseline: ReturnType<SkillWatcher["markBaseline"]>,
    driverTracksSkills: boolean,
    fail: (msg: string) => void,
  ): Promise<string[]> {
    let activatedSkills: string[];

    if (driverTracksSkills) {
      activatedSkills = this.driver.getActivatedSkills() ?? [];
      for (const name of activatedSkills) {
        log.stepSkillDetected(stepLabel, "driver", name, "");
      }
    } else {
      const watcherEvents = this.watcher.getActivatedEventsSince(baseline);
      const atimeResults = await this.watcher.getSkillsWithChangedAtime();
      for (const evt of watcherEvents) {
        log.stepSkillDetected(stepLabel, "fswatch", evt.skillName, evt.path);
      }
      for (const res of atimeResults) {
        log.stepSkillDetected(stepLabel, "atime", res.skillName, res.path);
      }
      activatedSkills = Array.from(
        new Set([
          ...watcherEvents.map((e) => e.skillName),
          ...atimeResults.map((r) => r.skillName),
        ]),
      );
    }

    log.stepActivatedSkills(stepLabel, activatedSkills);
    const error = this.assertSkillActivation(stepLabel, step, activatedSkills);
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
    let verificationFailed = !currentSuccess;
    const recordFailure = (msg: string) => {
      verificationFailed = true;
      fail(msg);
    };

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
          recordFailure(`EXPECTED_FILE_MISSING: ${relPath}`);
        }
      }

      if (missingCount === 0) {
        log.stepAction(stepLabel, `expected ${fileLabel} verified`);
      } else {
        log.stepAction(
          stepLabel,
          `expected ${fileLabel} verification failed (${missingCount} missing)`,
        );
      }
    }

    if (verify.build) {
      log.stepAction(stepLabel, `running golem build in ${projectDir}`);
      const result = await this.runLocalCommand("golem", ["build"], 600, projectDir, commandEnv);
      log.cliOutput(stepLabel, "golem build", result.output);
      if (!result.success) recordFailure(`BUILD_FAILED: ${result.output}`);
    }

    if (verify.deploy) {
      if (verificationFailed) {
        return;
      }

      // Deploy implies build — run build first if not already run
      if (!verify.build) {
        const golemTempDir = path.join(projectDir, "golem-temp");
        await fs.rm(golemTempDir, { recursive: true, force: true });
        log.stepAction(stepLabel, `running implicit golem build before deploy in ${projectDir}`);
        const buildResult = await this.runLocalCommand(
          "golem",
          ["build"],
          600,
          projectDir,
          commandEnv,
        );
        log.cliOutput(stepLabel, "golem build", buildResult.output);
        if (!buildResult.success) {
          recordFailure(`BUILD_FAILED: ${buildResult.output}`);
          return;
        }
      }

      if (!verificationFailed) {
        log.stepAction(stepLabel, `running golem deploy in ${projectDir}`);
        const deployResult = await this.runLocalCommand(
          "golem",
          ["deploy", "--yes"],
          600,
          projectDir,
          commandEnv,
        );
        log.cliOutput(stepLabel, "golem deploy", deployResult.output);
        if (!deployResult.success) recordFailure(`DEPLOY_FAILED: ${deployResult.output}`);
      }
    }
  }

  private assertSkillActivation(
    stepLabel: string,
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

    const allowedExtraSkills = step.allowedExtraSkills as string[] | undefined;
    if (allowedExtraSkills) {
      const allowedExtras = new Set(allowedExtraSkills);
      const unexpectedExtras = activatedSkills.filter(
        (skill) => !expectedSet.has(skill) && !allowedExtras.has(skill),
      );
      if (unexpectedExtras.length > 0) {
        log.warn(
          `extra skills activated beyond allowedExtraSkills: [${unexpectedExtras.join(", ")}]`,
        );
      }
    }

    return undefined;
  }

  private buildCommandEnv(
    spec: ScenarioSpec,
    serviceEnv?: Record<string, string>,
  ): Record<string, string> {
    const env: Record<string, string> = { ...(serviceEnv ?? {}) };
    if (spec.settings?.golem_server?.router_port) {
      env["GOLEM_ROUTER_PORT"] = String(spec.settings.golem_server.router_port);
    }
    if (spec.settings?.golem_server?.custom_request_port) {
      env["GOLEM_CUSTOM_REQUEST_PORT"] = String(spec.settings.golem_server.custom_request_port);
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
      throw new Error(`Failed to verify local Golem profile: ${profileCheck.output}`);
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
    onOutput?: (stream: "stdout" | "stderr", data: string) => void,
  ): Promise<LocalCommandResult> {
    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        detached: true,
        env: { ...process.env, ...extraEnv },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let stdout = "";
      let stderr = "";
      let timedOut = false;
      child.stdout?.on("data", (chunk) => {
        const text = chunk.toString();
        stdout += text;
        onOutput?.("stdout", text);
      });
      child.stderr?.on("data", (chunk) => {
        const text = chunk.toString();
        stderr += text;
        onOutput?.("stderr", text);
      });

      const timeoutId = setTimeout(() => {
        timedOut = true;
        killProcessTree(child);
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
        resolve({
          success: !timedOut && exitCode === 0,
          stdout,
          stderr,
          output: stdout + stderr,
          exitCode: timedOut ? null : exitCode,
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
