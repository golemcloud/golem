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

export const DEFAULT_STEP_TIMEOUT_SECONDS = 300;
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
  function: z.string(),
  args: z.string().optional(),
});

const ShellSchema = z.object({
  command: z.string(),
  args: z.array(z.string()).optional(),
  cwd: z.string().optional(),
});

const TriggerSchema = z.object({
  agent: z.string(),
  function: z.string(),
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
  "shell",
  "trigger",
  "create_agent",
  "delete_agent",
  "sleep",
  "http",
] as const;

const StepSpecSchema = z
  .object({
    id: z.string().optional(),
    prompt: z.string().optional(),
    expectedSkills: z.array(z.string()).optional(),
    allowedExtraSkills: z.array(z.string()).optional(),
    strictSkillMatch: z.boolean().optional(),
    timeout: z.number().optional(),
    continue_session: z.boolean().optional(),
    verify: z
      .object({
        build: z.boolean().optional(),
        deploy: z.boolean().optional(),
      })
      .optional(),
    invoke: InvokeSchema.optional(),
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
  steps: z.array(StepSpecSchema).min(1, "Scenario must have at least one step"),
});

interface StepCommon {
  id?: string;
  expectedSkills?: string[];
  allowedExtraSkills?: string[];
  strictSkillMatch?: boolean;
  timeout?: number;
  continue_session?: boolean;
  verify?: {
    build?: boolean;
    deploy?: boolean;
  };
  expect?: z.infer<typeof ExpectSchema>;
  only_if?: StepCondition;
  skip_if?: StepCondition;
  retry?: { attempts: number; delay: number };
}

type InvokeSpec = { agent: string; function: string; args?: string };
type ShellSpec = { command: string; args?: string[]; cwd?: string };
type TriggerSpec = { agent: string; function: string; args?: string };
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

export type StepSpec =
  | (StepCommon & {
      prompt: string;
      invoke?: undefined;
      shell?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      invoke: InvokeSpec;
      prompt?: undefined;
      shell?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      shell: ShellSpec;
      prompt?: undefined;
      invoke?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      trigger: TriggerSpec;
      prompt?: undefined;
      invoke?: undefined;
      shell?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      create_agent: CreateAgentSpec;
      prompt?: undefined;
      invoke?: undefined;
      shell?: undefined;
      trigger?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      delete_agent: DeleteAgentSpec;
      prompt?: undefined;
      invoke?: undefined;
      shell?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      sleep?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      sleep: number;
      prompt?: undefined;
      invoke?: undefined;
      shell?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      http?: undefined;
    })
  | (StepCommon & {
      http: HttpSpec;
      prompt?: undefined;
      invoke?: undefined;
      shell?: undefined;
      trigger?: undefined;
      create_agent?: undefined;
      delete_agent?: undefined;
      sleep?: undefined;
    });

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
  steps: StepSpec[];
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
    // The refine() guarantees exactly one action field, making the cast safe
    return result.data as unknown as ScenarioSpec;
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

export interface ScenarioExecutorOptions {
  globalTimeoutSeconds?: number;
  agent?: string;
  language?: string;
  abortSignal?: AbortSignal;
  resumeFromStepId?: string;
  skipCleanup?: boolean;
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
  step: StepSpec,
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

    return {
      ...step,
      prompt: sub(step.prompt),
      shell: step.shell
        ? {
            command: substituteVariables(step.shell.command, variables),
            args: subArr(step.shell.args),
            cwd: sub(step.shell.cwd),
          }
        : step.shell,
      invoke: step.invoke
        ? {
            agent: substituteVariables(step.invoke.agent, variables),
            function: substituteVariables(step.invoke.function, variables),
            args: sub(step.invoke.args),
          }
        : step.invoke,
      trigger: step.trigger
        ? {
            agent: substituteVariables(step.trigger.agent, variables),
            function: substituteVariables(step.trigger.function, variables),
            args: sub(step.trigger.args),
          }
        : step.trigger,
      create_agent: step.create_agent
        ? {
            ...step.create_agent,
            name: substituteVariables(step.create_agent.name, variables),
          }
        : step.create_agent,
      delete_agent: step.delete_agent
        ? {
            ...step.delete_agent,
            name: substituteVariables(step.delete_agent.name, variables),
          }
        : step.delete_agent,
      http: step.http
        ? {
            ...step.http,
            url: substituteVariables(step.http.url, variables),
            body: sub(step.http.body),
          }
        : step.http,
    } as StepSpec;
  }

  async execute(spec: ScenarioSpec): Promise<ScenarioRunResult> {
    const results: StepResult[] = [];
    const savedEnv: Record<string, string | undefined> = {};
    const shouldCleanup =
      spec.settings?.cleanup !== false && !this.options.skipCleanup;

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

    // Clean and setup workspace
    if (shouldCleanup) {
      await fs.rm(this.workspace, { recursive: true, force: true });
    }
    await fs.mkdir(this.workspace, { recursive: true });
    await this.driver.setup(this.workspace, this.skillsDir);
    // Watch all agent skill directories — each agent reads skills from its own location
    const agentSkillsDirs = [
      ".claude/skills",
      ".gemini/skills",
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

        // Substitute template variables
        const step = this.substituteStepVariables(originalStep, variables);

        // Resume-from: skip steps before the target
        if (!resumeReached) {
          if (step.id === this.options.resumeFromStepId) {
            resumeReached = true;
          } else {
            console.log(
              `Step ${step.id ?? "(unnamed)"}: skipped (before resume point)`,
            );
            results.push({
              step: originalStep,
              success: true,
              durationSeconds: 0,
              expectedSkills: step.expectedSkills ?? [],
              activatedSkills: [],
            });
            continue;
          }
        }

        // Conditional execution
        if (!shouldRunStep(step, conditionContext)) {
          console.log(
            `Step ${step.id ?? "(unnamed)"}: skipped (condition not met)`,
          );
          results.push({
            step: originalStep,
            success: true,
            durationSeconds: 0,
            expectedSkills: step.expectedSkills ?? [],
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
            console.log(
              `Step ${step.id ?? "(unnamed)"}: retry attempt ${attempt}/${maxAttempts} (delay=${retryDelay}s)`,
            );
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
          expectedSkills: step.expectedSkills ?? [],
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
    let stepSuccess = true;
    const stepErrors: string[] = [];
    const stepTimeoutSeconds =
      step.timeout ??
      spec.settings?.timeout_per_subprompt ??
      this.options.globalTimeoutSeconds ??
      DEFAULT_STEP_TIMEOUT_SECONDS;
    const stepBaseline = this.watcher.markBaseline();
    await this.watcher.snapshotAtimes();
    console.log(
      `Step ${step.id ?? "(unnamed)"}: starting (timeout=${stepTimeoutSeconds}s)`,
    );

    // Sleep action
    if (step.sleep !== undefined) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: sleeping for ${step.sleep}s`,
      );
      await new Promise((resolve) => setTimeout(resolve, step.sleep! * 1000));
    }

    // Create agent action
    if (step.create_agent) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: creating agent "${step.create_agent.name}"`,
      );
      const createResult = await this.runLocalCommand(
        "golem",
        ["agent", "new", step.create_agent.name],
        stepTimeoutSeconds,
        this.workspace,
        commandEnv,
      );
      if (!createResult.success) {
        stepSuccess = false;
        stepErrors.push(`CREATE_AGENT_FAILED: ${createResult.output}`);
      }
    }

    // Delete agent action
    if (step.delete_agent) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: deleting agent "${step.delete_agent.name}"`,
      );
      const deleteResult = await this.runLocalCommand(
        "golem",
        ["agent", "delete", step.delete_agent.name],
        stepTimeoutSeconds,
        this.workspace,
        commandEnv,
      );
      if (!deleteResult.success) {
        stepSuccess = false;
        stepErrors.push(`DELETE_AGENT_FAILED: ${deleteResult.output}`);
      }
    }

    // Shell action
    if (step.shell) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: running shell command "${step.shell.command}"`,
      );
      const shellCwd = step.shell.cwd
        ? path.resolve(this.workspace, step.shell.cwd)
        : this.workspace;
      const shellResult = await this.runLocalCommand(
        step.shell.command,
        step.shell.args ?? [],
        stepTimeoutSeconds,
        shellCwd,
        commandEnv,
      );

      if (step.expect) {
        const ctx: AssertionContext = {
          stdout: shellResult.output,
          stderr: "",
          exitCode: shellResult.exitCode,
        };
        const assertionResults = evaluate(ctx, step.expect);
        for (const ar of assertionResults) {
          if (!ar.passed) {
            stepSuccess = false;
            stepErrors.push(
              `ASSERTION_FAILED (${ar.assertion}): ${ar.message}`,
            );
          }
        }
      } else if (!shellResult.success) {
        stepSuccess = false;
        stepErrors.push(`SHELL_FAILED: ${shellResult.output}`);
      }
    }

    // Trigger action — fire-and-forget
    if (step.trigger) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: triggering ${step.trigger.agent}.${step.trigger.function}`,
      );
      const triggerArgs = [
        "agent",
        "invoke",
        step.trigger.agent,
        step.trigger.function,
        "--trigger",
      ];
      if (step.trigger.args) triggerArgs.push(step.trigger.args);
      // Fire and forget — don't await completion
      this.runLocalCommand(
        "golem",
        triggerArgs,
        stepTimeoutSeconds,
        this.workspace,
        commandEnv,
      ).catch(() => {
        /* fire and forget */
      });
    }

    // Execute prompt if present
    if (step.prompt) {
      const useContinueSession =
        step.continue_session !== false && !isFirstPrompt;
      if (useContinueSession) {
        console.log(`Step ${step.id ?? "(unnamed)"}: sending followup prompt`);
        const agentResult = await this.driver.sendFollowup(
          step.prompt,
          stepTimeoutSeconds,
        );
        if (!agentResult.success) {
          stepSuccess = false;
          stepErrors.push(`Agent failed: ${agentResult.output}`);
        }
      } else {
        console.log(`Step ${step.id ?? "(unnamed)"}: sending prompt`);
        const agentResult = await this.driver.sendPrompt(
          step.prompt,
          stepTimeoutSeconds,
        );
        if (!agentResult.success) {
          stepSuccess = false;
          stepErrors.push(`Agent failed: ${agentResult.output}`);
        }
      }
      isFirstPrompt = false;
    }

    // Invoke action
    if (step.invoke) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: invoking ${step.invoke.agent}.${step.invoke.function}`,
      );
      const invokeArgs = [
        "agent",
        "invoke",
        step.invoke.agent,
        step.invoke.function,
      ];
      if (step.invoke.args) invokeArgs.push(step.invoke.args);
      const invokeResult = await this.runLocalCommand(
        "golem",
        invokeArgs,
        stepTimeoutSeconds,
        this.workspace,
        commandEnv,
      );

      if (step.expect) {
        let resultJson: unknown = undefined;
        try {
          resultJson = JSON.parse(invokeResult.output);
        } catch {
          // Not JSON — that's fine
        }
        const ctx: AssertionContext = {
          stdout: invokeResult.output,
          stderr: "",
          exitCode: invokeResult.exitCode,
          resultJson,
        };
        const assertionResults = evaluate(ctx, step.expect);
        for (const ar of assertionResults) {
          if (!ar.passed) {
            stepSuccess = false;
            stepErrors.push(
              `ASSERTION_FAILED (${ar.assertion}): ${ar.message}`,
            );
          }
        }
      } else if (!invokeResult.success) {
        stepSuccess = false;
        stepErrors.push(`INVOKE_FAILED: ${invokeResult.output}`);
      }
    }

    // HTTP behavioral check
    if (step.http) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: HTTP ${step.http.method ?? "GET"} ${step.http.url}`,
      );
      try {
        const response = await fetch(step.http.url, {
          method: step.http.method ?? "GET",
          body: step.http.body,
          headers: step.http.headers,
        });
        const body = await response.text();

        if (step.expect) {
          const ctx: AssertionContext = {
            stdout: body,
            stderr: "",
            exitCode: response.ok ? 0 : 1,
            body,
            status: response.status,
          };
          const assertionResults = evaluate(ctx, step.expect);
          for (const ar of assertionResults) {
            if (!ar.passed) {
              stepSuccess = false;
              stepErrors.push(
                `ASSERTION_FAILED (${ar.assertion}): ${ar.message}`,
              );
            }
          }
        } else if (!response.ok) {
          stepSuccess = false;
          stepErrors.push(
            `HTTP_FAILED: ${response.status} ${body.slice(0, 500)}`,
          );
        }
      } catch (err) {
        stepSuccess = false;
        stepErrors.push(
          `HTTP_FAILED: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }

    // Verify skills activation — merge fswatch events + atime changes
    const watcherEvents = this.watcher.getActivatedEventsSince(stepBaseline);
    const atimeResults = await this.watcher.getSkillsWithChangedAtime();
    for (const evt of watcherEvents) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: fswatch detected "${evt.skillName}" via ${evt.path}`,
      );
    }
    for (const res of atimeResults) {
      console.log(
        `Step ${step.id ?? "(unnamed)"}: atime detected "${res.skillName}" via ${res.path}`,
      );
    }
    const activatedSkills = Array.from(
      new Set([
        ...watcherEvents.map((e) => e.skillName),
        ...atimeResults.map((r) => r.skillName),
      ]),
    );
    console.log(
      `Step ${step.id ?? "(unnamed)"}: activated skills [${activatedSkills.join(", ")}]`,
    );
    const assertionError = this.assertSkillActivation(step, activatedSkills);
    if (assertionError) {
      stepSuccess = false;
      stepErrors.push(assertionError);
    }

    // Build verification
    if (step.verify?.build) {
      const buildDir = await this.findGolemProjectDir();
      console.log(
        `Step ${step.id ?? "(unnamed)"}: running golem build in ${buildDir}`,
      );
      const buildResult = await this.runLocalCommand(
        "golem",
        ["build"],
        600,
        buildDir,
        commandEnv,
      );
      if (!buildResult.success) {
        stepSuccess = false;
        stepErrors.push(`BUILD_FAILED: ${buildResult.output}`);
      }
    }

    // Deploy verification
    if (step.verify?.deploy) {
      // Deploy implies build — run build first if not already run
      if (!step.verify?.build) {
        const buildDir = await this.findGolemProjectDir();
        // Clean golem-temp to force a fresh build — the agent may have built
        // with different SDK paths, leaving stale cached artifacts
        const golemTempDir = path.join(buildDir, "golem-temp");
        await fs.rm(golemTempDir, { recursive: true, force: true });
        console.log(
          `Step ${step.id ?? "(unnamed)"}: running implicit golem build before deploy in ${buildDir}`,
        );
        const buildResult = await this.runLocalCommand(
          "golem",
          ["build"],
          600,
          buildDir,
          commandEnv,
        );
        if (!buildResult.success) {
          stepSuccess = false;
          stepErrors.push(`BUILD_FAILED: ${buildResult.output}`);
        }
      }

      if (stepSuccess) {
        const deployDir = await this.findGolemProjectDir();
        console.log(
          `Step ${step.id ?? "(unnamed)"}: running golem deploy in ${deployDir}`,
        );
        const deployResult = await this.runLocalCommand(
          "golem",
          ["deploy", "--yes"],
          600,
          deployDir,
          commandEnv,
        );
        if (!deployResult.success) {
          stepSuccess = false;
          stepErrors.push(`DEPLOY_FAILED: ${deployResult.output}`);
        }
      }
    }

    return {
      success: stepSuccess,
      errors: stepErrors,
      activatedSkills,
      isFirstPrompt,
    };
  }

  private assertSkillActivation(
    step: StepSpec,
    activatedSkills: string[],
  ): string | undefined {
    const expectedSkills = step.expectedSkills ?? [];
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

    const allowedExtras = new Set(step.allowedExtraSkills ?? []);
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
    // Check workspace root first
    try {
      await fs.access(path.join(this.workspace, "golem.yaml"));
      return this.workspace;
    } catch {
      // Not in root, search immediate subdirectories
    }

    const entries = await fs.readdir(this.workspace, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      const candidate = path.join(this.workspace, entry.name);
      try {
        await fs.access(path.join(candidate, "golem.yaml"));
        return candidate;
      } catch {
        // Continue searching
      }
    }

    // Fall back to workspace root and let golem build report the error
    return this.workspace;
  }

  private async runLocalCommand(
    command: string,
    args: string[],
    timeoutSeconds: number,
    cwd: string,
    extraEnv?: Record<string, string>,
  ): Promise<{ success: boolean; output: string; exitCode: number | null }> {
    const controller = new AbortController();
    const { signal } = controller;

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        signal,
        env: { ...process.env, ...extraEnv },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let output = "";
      child.stdout?.on("data", (data) => (output += data.toString()));
      child.stderr?.on("data", (data) => (output += data.toString()));

      const timeoutId = setTimeout(() => {
        controller.abort();
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
        resolve({ success: exitCode === 0, output, exitCode });
      });

      child.on("error", (error) => {
        clearTimeout(timeoutId);
        resolve({
          success: false,
          output: output + error.message,
          exitCode: null,
        });
      });
    });
  }
}
