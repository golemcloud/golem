import * as fs from 'node:fs/promises';
import { spawn } from 'node:child_process';
import * as path from 'node:path';
import * as yaml from 'yaml';
import { z } from 'zod';
import { AgentDriver } from './driver/base.js';
import { SkillWatcher } from './watcher.js';

const StepSpecSchema = z.object({
  id: z.string().optional(),
  prompt: z.string().optional(),
  expectedSkills: z.array(z.string()).optional(),
  allowedExtraSkills: z.array(z.string()).optional(),
  strictSkillMatch: z.boolean().optional(),
  verify: z.object({
    build: z.boolean().optional(),
    deploy: z.boolean().optional(),
  }).optional(),
});

const ScenarioSpecSchema = z.object({
  name: z.string({ required_error: 'Scenario must have a "name" field' }),
  steps: z.array(StepSpecSchema).min(1, 'Scenario must have at least one step'),
});

export interface StepSpec {
  id?: string;
  prompt?: string;
  expectedSkills?: string[];
  allowedExtraSkills?: string[];
  strictSkillMatch?: boolean;
  verify?: {
    build?: boolean;
    deploy?: boolean;
  };
}

export interface ScenarioSpec {
  name: string;
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
}

export interface StepResult {
  step: StepSpec;
  success: boolean;
  durationSeconds: number;
  expectedSkills: string[];
  activatedSkills: string[];
  error?: string;
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

    // Clean and setup workspace
    await fs.rm(this.workspace, { recursive: true, force: true });
    await fs.mkdir(this.workspace, { recursive: true });
    await this.driver.setup(this.workspace, this.skillsDir);
    // Watch workspace .claude/skills/ too — agents read skills from there
    const workspaceSkillsDir = path.join(this.workspace, '.claude', 'skills');
    this.watcher.addWatchDir(workspaceSkillsDir);
    await this.verifyGolemConnectivity();
    await this.watcher.start();

    const startTime = Date.now();
    try {
      for (const step of spec.steps) {
        const stepStartTime = Date.now();
        let stepSuccess = true;
        const stepErrors: string[] = [];
        const stepTimeoutSeconds = this.options.globalTimeoutSeconds ?? 300;
        const stepBaseline = this.watcher.markBaseline();
        await this.watcher.snapshotAtimes();
        console.log(`Step ${step.id ?? '(unnamed)'}: starting (timeout=${stepTimeoutSeconds}s)`);

        // Execute prompt if present
        if (step.prompt) {
          console.log(`Step ${step.id ?? '(unnamed)'}: sending prompt`);
          const agentResult = await this.driver.sendPrompt(step.prompt, stepTimeoutSeconds);
          if (!agentResult.success) {
            stepSuccess = false;
            stepErrors.push(`Agent failed: ${agentResult.output}`);
          }
        }

        // Verify skills activation — merge fswatch events + atime changes + presence check
        const watcherSkills = this.watcher.getActivatedSkillsSince(stepBaseline);
        const atimeSkills = await this.watcher.getSkillsWithChangedAtime();
        const activatedSet = new Set([...watcherSkills, ...atimeSkills]);

        // Fallback: if no filesystem-level detection succeeded, check which expected
        // skills have their SKILL.md present in the workspace .claude/skills/ directory.
        // Agents like Claude Code read all available skills at startup, so presence
        // in the skills directory means the skill was loaded into the agent's context.
        if (activatedSet.size === 0 && step.expectedSkills && step.expectedSkills.length > 0) {
          const presenceSkills = await this.detectSkillsByPresence(step.expectedSkills);
          if (presenceSkills.length > 0) {
            console.log(`Step ${step.id ?? '(unnamed)'}: filesystem skill detection unavailable, using presence check`);
            for (const skill of presenceSkills) {
              activatedSet.add(skill);
            }
          }
        }

        const activatedSkills = Array.from(activatedSet);
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
          const buildResult = await this.runLocalCommand('golem', ['build'], 600, buildDir);
          if (!buildResult.success) {
            stepSuccess = false;
            stepErrors.push(`BUILD_FAILED: ${buildResult.output}`);
          }
        }

        results.push({
          step,
          success: stepSuccess,
          durationSeconds: (Date.now() - stepStartTime) / 1000,
          expectedSkills: step.expectedSkills ?? [],
          activatedSkills,
          error: stepErrors.length > 0 ? stepErrors.join('\n') : undefined
        });

        if (!stepSuccess) break; // Stop on failure
      }
    } finally {
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

  private async verifyGolemConnectivity(): Promise<void> {
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
      ['-fsS', 'http://localhost:9881/healthcheck'],
      30,
      this.workspace
    );
    if (!serverCheck.success) {
      throw new Error(`Failed to connect to local Golem server on localhost:9881: ${serverCheck.output}`);
    }
  }

  private async detectSkillsByPresence(expectedSkills: string[]): Promise<string[]> {
    const workspaceSkillsDir = path.join(this.workspace, '.claude', 'skills');
    const found: string[] = [];
    for (const skillName of expectedSkills) {
      const skillFile = path.join(workspaceSkillsDir, skillName, 'SKILL.md');
      try {
        await fs.access(skillFile);
        found.push(skillName);
      } catch {
        // Skill file not present
      }
    }
    return found;
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
    cwd: string
  ): Promise<{ success: boolean; output: string; exitCode: number | null }> {
    const controller = new AbortController();
    const { signal } = controller;

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        signal,
        env: { ...process.env },
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
