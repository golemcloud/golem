import * as fs from 'node:fs/promises';
import { spawn } from 'node:child_process';
import * as path from 'node:path';
import * as yaml from 'yaml';
import { z } from 'zod';
import { AgentDriver } from './driver/base.js';
import { SkillWatcher } from './watcher.js';
import { evaluate, ExpectSchema, type AssertionContext } from './assertions.js';

// --- Schemas (#2887, #2890, #2893, #2894, #2911, #2916) ---

const MatrixSchema = z.object({
  agents: z.array(z.string()).optional(),
  languages: z.array(z.string()).optional(),
  os: z.array(z.string()).optional(),
});

const RetrySchema = z.object({
  attempts: z.number().int().min(1),
  delay: z.number().min(0),
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

const StepSpecSchema = z.object({
  id: z.string().optional(),
  prompt: z.string().optional(),
  expectedSkills: z.array(z.string()).optional(),
  allowedExtraSkills: z.array(z.string()).optional(),
  strictSkillMatch: z.boolean().optional(),
  timeout: z.number().optional(),
  continue_session: z.boolean().optional(),
  verify: z.object({
    build: z.boolean().optional(),
    deploy: z.boolean().optional(),
  }).optional(),
  invoke: InvokeSchema.optional(),
  expect: ExpectSchema.optional(),
  sleep: z.number().optional(),
  shell: ShellSchema.optional(),
  trigger: TriggerSchema.optional(),
  create_agent: CreateAgentSchema.optional(),
  delete_agent: DeleteAgentSchema.optional(),
  retry: RetrySchema.optional(),
});

const SettingsSchema = z.object({
  timeout_per_subprompt: z.number().optional(),
  golem_server: z.object({
    router_port: z.number().optional(),
    custom_request_port: z.number().optional(),
  }).optional(),
  cleanup: z.boolean().optional(),
}).optional();

const PrerequisitesSchema = z.object({
  env: z.record(z.string()).optional(),
}).optional();

const ScenarioSpecSchema = z.object({
  name: z.string({ required_error: 'Scenario must have a "name" field' }),
  settings: SettingsSchema,
  prerequisites: PrerequisitesSchema,
  matrix: MatrixSchema.optional(),
  steps: z.array(StepSpecSchema).min(1, 'Scenario must have at least one step'),
});

export interface StepSpec {
  id?: string;
  prompt?: string;
  expectedSkills?: string[];
  allowedExtraSkills?: string[];
  strictSkillMatch?: boolean;
  timeout?: number;
  continue_session?: boolean;
  verify?: {
    build?: boolean;
    deploy?: boolean;
  };
  invoke?: {
    agent: string;
    function: string;
    args?: string;
  };
  expect?: z.infer<typeof ExpectSchema>;
  sleep?: number;
  shell?: {
    command: string;
    args?: string[];
    cwd?: string;
  };
  trigger?: {
    agent: string;
    function: string;
    args?: string;
  };
  create_agent?: {
    name: string;
    env?: Record<string, string>;
    config?: Record<string, string>;
  };
  delete_agent?: {
    name: string;
  };
  retry?: {
    attempts: number;
    delay: number;
  };
}

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
  matrix?: {
    agents?: string[];
    languages?: string[];
    os?: string[];
  };
  steps: StepSpec[];
}

export class ScenarioLoader {
  static async load(filePath: string): Promise<ScenarioSpec> {
    const content = await fs.readFile(filePath, 'utf8');
    const raw = yaml.parse(content);
    const result = ScenarioSpecSchema.safeParse(raw);
    if (!result.success) {
      const issues = result.error.issues
        .map((i) => `  - ${i.path.join('.')}: ${i.message}`)
        .join('\n');
      throw new Error(`Invalid scenario file "${filePath}":\n${issues}`);
    }
    return result.data;
  }

  static matchesMatrix(spec: ScenarioSpec, agent: string, language: string): boolean {
    if (!spec.matrix) return true;
    const currentOs = process.platform;
    if (spec.matrix.agents && !spec.matrix.agents.includes(agent)) return false;
    if (spec.matrix.languages && !spec.matrix.languages.includes(language)) return false;
    if (spec.matrix.os && !spec.matrix.os.includes(currentOs)) return false;
    return true;
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
}

export interface ScenarioRunResult {
  status: 'pass' | 'fail';
  durationSeconds: number;
  stepResults: StepResult[];
  artifactPaths: string[];
  workspace: string;
}

export interface ScenarioExecutorOptions {
  globalTimeoutSeconds?: number;
  resumeFromStepId?: string;
  skipCleanup?: boolean;
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
    options?: ScenarioExecutorOptions
  ) {
    this.driver = driver;
    this.watcher = watcher;
    this.workspace = workspace;
    this.skillsDir = skillsDir;
    this.options = options ?? {};
  }

  async execute(spec: ScenarioSpec): Promise<ScenarioRunResult> {
    const results: StepResult[] = [];
    const savedEnv: Record<string, string | undefined> = {};
    const shouldCleanup = spec.settings?.cleanup !== false && !this.options.skipCleanup;

    // Validate resumeFromStepId if set (#2915)
    if (this.options.resumeFromStepId) {
      const found = spec.steps.some(s => s.id === this.options.resumeFromStepId);
      if (!found) {
        throw new Error(`Resume step "${this.options.resumeFromStepId}" not found in scenario "${spec.name}"`);
      }
    }

    // Set prerequisites env vars (#2887)
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
    const agentSkillsDirs = ['.claude/skills', '.gemini/skills', '.agents/skills'];
    for (const rel of agentSkillsDirs) {
      this.watcher.addWatchDir(path.join(this.workspace, rel));
    }
    await this.verifyGolemConnectivity(spec);
    await this.watcher.start();

    // Build extra env for commands from settings
    const commandEnv = this.buildCommandEnv(spec);

    const startTime = Date.now();
    let isFirstPrompt = true;
    let resumeReached = !this.options.resumeFromStepId;
    try {
      for (const step of spec.steps) {
        // Resume-from: skip steps before the target (#2915)
        if (!resumeReached) {
          if (step.id === this.options.resumeFromStepId) {
            resumeReached = true;
          } else {
            console.log(`Step ${step.id ?? '(unnamed)'}: skipped (before resume point)`);
            results.push({
              step,
              success: true,
              durationSeconds: 0,
              expectedSkills: step.expectedSkills ?? [],
              activatedSkills: [],
            });
            continue;
          }
        }

        const maxAttempts = step.retry?.attempts ?? 1;
        const retryDelay = step.retry?.delay ?? 0;
        const attempts: StepAttemptResult[] = [];
        let finalResult: { success: boolean; errors: string[]; activatedSkills: string[]; isFirstPrompt: boolean } | undefined;

        for (let attempt = 1; attempt <= maxAttempts; attempt++) {
          if (attempt > 1) {
            console.log(`Step ${step.id ?? '(unnamed)'}: retry attempt ${attempt}/${maxAttempts} (delay=${retryDelay}s)`);
            await new Promise(resolve => setTimeout(resolve, retryDelay * 1000));
          }

          const attemptStart = Date.now();
          const bodyResult = await this.executeStepBody(step, spec, commandEnv, isFirstPrompt);
          const attemptDuration = (Date.now() - attemptStart) / 1000;
          isFirstPrompt = bodyResult.isFirstPrompt;

          attempts.push({
            attemptNumber: attempt,
            success: bodyResult.success,
            durationSeconds: attemptDuration,
            error: bodyResult.errors.length > 0 ? bodyResult.errors.join('\n') : undefined,
            activatedSkills: bodyResult.activatedSkills,
          });

          finalResult = bodyResult;
          if (bodyResult.success) break;
        }

        const stepStartTime = attempts.reduce((sum, a) => sum + a.durationSeconds, 0);
        results.push({
          step,
          success: finalResult!.success,
          durationSeconds: stepStartTime,
          expectedSkills: step.expectedSkills ?? [],
          activatedSkills: finalResult!.activatedSkills,
          error: finalResult!.errors.length > 0 ? finalResult!.errors.join('\n') : undefined,
          attempts: maxAttempts > 1 ? attempts : undefined,
        });

        if (!finalResult!.success) break; // Stop on failure
      }
    } finally {
      // Restore env vars (#2887)
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
      status: results.every((result) => result.success) ? 'pass' : 'fail',
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
    isFirstPrompt: boolean
  ): Promise<{ success: boolean; errors: string[]; activatedSkills: string[]; isFirstPrompt: boolean }> {
    let stepSuccess = true;
    const stepErrors: string[] = [];
    const stepTimeoutSeconds = step.timeout
      ?? spec.settings?.timeout_per_subprompt
      ?? this.options.globalTimeoutSeconds
      ?? 300;
    const stepBaseline = this.watcher.markBaseline();
    await this.watcher.snapshotAtimes();
    console.log(`Step ${step.id ?? '(unnamed)'}: starting (timeout=${stepTimeoutSeconds}s)`);

    // Sleep action (#2893)
    if (step.sleep !== undefined) {
      console.log(`Step ${step.id ?? '(unnamed)'}: sleeping for ${step.sleep}s`);
      await new Promise(resolve => setTimeout(resolve, step.sleep! * 1000));
    }

    // Create agent action (#2894)
    if (step.create_agent) {
      console.log(`Step ${step.id ?? '(unnamed)'}: creating agent "${step.create_agent.name}"`);
      const createResult = await this.runLocalCommand(
        'golem', ['agent', 'new', step.create_agent.name],
        stepTimeoutSeconds, this.workspace, commandEnv
      );
      if (!createResult.success) {
        stepSuccess = false;
        stepErrors.push(`CREATE_AGENT_FAILED: ${createResult.output}`);
      }
    }

    // Delete agent action (#2894)
    if (step.delete_agent) {
      console.log(`Step ${step.id ?? '(unnamed)'}: deleting agent "${step.delete_agent.name}"`);
      const deleteResult = await this.runLocalCommand(
        'golem', ['agent', 'delete', step.delete_agent.name],
        stepTimeoutSeconds, this.workspace, commandEnv
      );
      if (!deleteResult.success) {
        stepSuccess = false;
        stepErrors.push(`DELETE_AGENT_FAILED: ${deleteResult.output}`);
      }
    }

    // Shell action (#2893)
    if (step.shell) {
      console.log(`Step ${step.id ?? '(unnamed)'}: running shell command "${step.shell.command}"`);
      const shellCwd = step.shell.cwd
        ? path.resolve(this.workspace, step.shell.cwd)
        : this.workspace;
      const shellResult = await this.runLocalCommand(
        step.shell.command, step.shell.args ?? [],
        stepTimeoutSeconds, shellCwd, commandEnv
      );

      if (step.expect) {
        const ctx: AssertionContext = {
          stdout: shellResult.output,
          stderr: '',
          exitCode: shellResult.exitCode,
        };
        const assertionResults = evaluate(ctx, step.expect);
        for (const ar of assertionResults) {
          if (!ar.passed) {
            stepSuccess = false;
            stepErrors.push(`ASSERTION_FAILED (${ar.assertion}): ${ar.message}`);
          }
        }
      } else if (!shellResult.success) {
        stepSuccess = false;
        stepErrors.push(`SHELL_FAILED: ${shellResult.output}`);
      }
    }

    // Trigger action (#2893) — fire-and-forget
    if (step.trigger) {
      console.log(`Step ${step.id ?? '(unnamed)'}: triggering ${step.trigger.agent}.${step.trigger.function}`);
      const triggerArgs = ['agent', 'invoke', step.trigger.agent, step.trigger.function, '--trigger'];
      if (step.trigger.args) triggerArgs.push(step.trigger.args);
      // Fire and forget — don't await completion
      this.runLocalCommand('golem', triggerArgs, stepTimeoutSeconds, this.workspace, commandEnv)
        .catch(() => { /* fire and forget */ });
    }

    // Execute prompt if present (#2887 continue_session)
    if (step.prompt) {
      const useContinueSession = step.continue_session !== false && !isFirstPrompt;
      if (useContinueSession) {
        console.log(`Step ${step.id ?? '(unnamed)'}: sending followup prompt`);
        const agentResult = await this.driver.sendFollowup(step.prompt, stepTimeoutSeconds);
        if (!agentResult.success) {
          stepSuccess = false;
          stepErrors.push(`Agent failed: ${agentResult.output}`);
        }
      } else {
        console.log(`Step ${step.id ?? '(unnamed)'}: sending prompt`);
        const agentResult = await this.driver.sendPrompt(step.prompt, stepTimeoutSeconds);
        if (!agentResult.success) {
          stepSuccess = false;
          stepErrors.push(`Agent failed: ${agentResult.output}`);
        }
      }
      isFirstPrompt = false;
    }

    // Invoke action (#2890)
    if (step.invoke) {
      console.log(`Step ${step.id ?? '(unnamed)'}: invoking ${step.invoke.agent}.${step.invoke.function}`);
      const invokeArgs = ['agent', 'invoke', step.invoke.agent, step.invoke.function];
      if (step.invoke.args) invokeArgs.push(step.invoke.args);
      const invokeResult = await this.runLocalCommand(
        'golem', invokeArgs, stepTimeoutSeconds, this.workspace, commandEnv
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
          stderr: '',
          exitCode: invokeResult.exitCode,
          resultJson,
        };
        const assertionResults = evaluate(ctx, step.expect);
        for (const ar of assertionResults) {
          if (!ar.passed) {
            stepSuccess = false;
            stepErrors.push(`ASSERTION_FAILED (${ar.assertion}): ${ar.message}`);
          }
        }
      } else if (!invokeResult.success) {
        stepSuccess = false;
        stepErrors.push(`INVOKE_FAILED: ${invokeResult.output}`);
      }
    }

    // Verify skills activation — merge fswatch events + atime changes
    const watcherEvents = this.watcher.getActivatedEventsSince(stepBaseline);
    const atimeResults = await this.watcher.getSkillsWithChangedAtime();
    for (const evt of watcherEvents) {
      console.log(`Step ${step.id ?? '(unnamed)'}: fswatch detected "${evt.skillName}" via ${evt.path}`);
    }
    for (const res of atimeResults) {
      console.log(`Step ${step.id ?? '(unnamed)'}: atime detected "${res.skillName}" via ${res.path}`);
    }
    const activatedSkills = Array.from(new Set([
      ...watcherEvents.map(e => e.skillName),
      ...atimeResults.map(r => r.skillName),
    ]));
    console.log(`Step ${step.id ?? '(unnamed)'}: activated skills [${activatedSkills.join(', ')}]`);
    const assertionError = this.assertSkillActivation(step, activatedSkills);
    if (assertionError) {
      stepSuccess = false;
      stepErrors.push(assertionError);
    }

    // Build verification
    if (step.verify?.build) {
      const buildDir = await this.findGolemProjectDir();
      console.log(`Step ${step.id ?? '(unnamed)'}: running golem build in ${buildDir}`);
      const buildResult = await this.runLocalCommand('golem', ['build'], 600, buildDir, commandEnv);
      if (!buildResult.success) {
        stepSuccess = false;
        stepErrors.push(`BUILD_FAILED: ${buildResult.output}`);
      }
    }

    // Deploy verification (#2889)
    if (step.verify?.deploy) {
      // Deploy implies build — run build first if not already run
      if (!step.verify?.build) {
        const buildDir = await this.findGolemProjectDir();
        console.log(`Step ${step.id ?? '(unnamed)'}: running implicit golem build before deploy in ${buildDir}`);
        const buildResult = await this.runLocalCommand('golem', ['build'], 600, buildDir, commandEnv);
        if (!buildResult.success) {
          stepSuccess = false;
          stepErrors.push(`BUILD_FAILED: ${buildResult.output}`);
        }
      }

      if (stepSuccess) {
        const deployDir = await this.findGolemProjectDir();
        console.log(`Step ${step.id ?? '(unnamed)'}: running golem deploy in ${deployDir}`);
        const deployResult = await this.runLocalCommand(
          'golem', ['deploy', '--yes'], 600, deployDir, commandEnv
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

  private assertSkillActivation(step: StepSpec, activatedSkills: string[]): string | undefined {
    const expectedSkills = step.expectedSkills ?? [];
    if (expectedSkills.length === 0) {
      return undefined;
    }

    const expectedSet = new Set(expectedSkills);
    const activatedSet = new Set(activatedSkills);

    for (const expected of expectedSet) {
      if (!activatedSet.has(expected)) {
        return `SKILL_NOT_ACTIVATED: expected "${expected}" but activated [${activatedSkills.join(', ')}]`;
      }
    }

    if (step.strictSkillMatch) {
      const extras = activatedSkills.filter((skill) => !expectedSet.has(skill));
      if (extras.length > 0) {
        return `SKILL_MISMATCH: strict match enabled; unexpected skills [${extras.join(', ')}]`;
      }
      return undefined;
    }

    const allowedExtras = new Set(step.allowedExtraSkills ?? []);
    const unexpectedExtras = activatedSkills.filter(
      (skill) => !expectedSet.has(skill) && !allowedExtras.has(skill)
    );
    if (unexpectedExtras.length > 0) {
      return `SKILL_MISMATCH: unexpected extra skills [${unexpectedExtras.join(', ')}]`;
    }

    return undefined;
  }

  private buildCommandEnv(spec: ScenarioSpec): Record<string, string> {
    const env: Record<string, string> = {};
    if (spec.settings?.golem_server?.router_port) {
      env['GOLEM_ROUTER_PORT'] = String(spec.settings.golem_server.router_port);
    }
    if (spec.settings?.golem_server?.custom_request_port) {
      env['GOLEM_CUSTOM_REQUEST_PORT'] = String(spec.settings.golem_server.custom_request_port);
    }
    if (spec.prerequisites?.env) {
      Object.assign(env, spec.prerequisites.env);
    }
    return env;
  }

  private async verifyGolemConnectivity(spec?: ScenarioSpec): Promise<void> {
    const routerPort = spec?.settings?.golem_server?.router_port ?? 9881;

    const profileCheck = await this.runLocalCommand(
      'golem',
      ['profile', 'get', '--profile', 'local'],
      30,
      this.workspace
    );
    if (!profileCheck.success) {
      throw new Error(`Failed to verify local Golem profile: ${profileCheck.output}`);
    }

    const serverCheck = await this.runLocalCommand(
      'curl',
      ['-fsS', `http://localhost:${routerPort}/healthcheck`],
      30,
      this.workspace
    );
    if (!serverCheck.success) {
      throw new Error(`Failed to connect to local Golem server on localhost:${routerPort}: ${serverCheck.output}`);
    }
  }

  private async findGolemProjectDir(): Promise<string> {
    // Check workspace root first
    try {
      await fs.access(path.join(this.workspace, 'golem.yaml'));
      return this.workspace;
    } catch {
      // Not in root, search immediate subdirectories
    }

    const entries = await fs.readdir(this.workspace, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
      const candidate = path.join(this.workspace, entry.name);
      try {
        await fs.access(path.join(candidate, 'golem.yaml'));
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
    extraEnv?: Record<string, string>
  ): Promise<{ success: boolean; output: string; exitCode: number | null }> {
    const controller = new AbortController();
    const { signal } = controller;

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        signal,
        env: { ...process.env, ...extraEnv },
        stdio: ['ignore', 'pipe', 'pipe'],
      });

      let output = '';
      child.stdout?.on('data', (data) => (output += data.toString()));
      child.stderr?.on('data', (data) => (output += data.toString()));

      const timeoutId = setTimeout(() => {
        controller.abort();
      }, timeoutSeconds * 1000);

      child.on('close', (exitCode) => {
        clearTimeout(timeoutId);
        resolve({ success: exitCode === 0, output, exitCode });
      });

      child.on('error', (error) => {
        clearTimeout(timeoutId);
        resolve({ success: false, output: output + error.message, exitCode: null });
      });
    });
  }
}
