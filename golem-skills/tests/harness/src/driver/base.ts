import { spawn, type ChildProcess } from "node:child_process";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as log from "../log.js";

/**
 * Kill a child process and its entire process tree.
 * Uses negative PID to send SIGTERM to the process group, falling back to
 * killing just the process. Schedules a SIGKILL if the tree doesn't exit
 * within 5 seconds.
 */
export function killProcessTree(child: ChildProcess): void {
  try {
    if (child.pid) {
      process.kill(-child.pid, "SIGTERM");
    } else {
      child.kill("SIGTERM");
    }
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {
      // Already dead
    }
  }

  setTimeout(() => {
    try {
      if (child.pid) {
        process.kill(-child.pid, "SIGKILL");
      } else {
        child.kill("SIGKILL");
      }
    } catch {
      // Already dead
    }
  }, 5000);
}

export interface AgentResult {
  success: boolean;
  output: string;
  durationSeconds: number;
  exitCode: number | null;
}

export interface AgentDriver {
  setup(workspace: string, bootstrapSkillSourceDir: string): Promise<void>;
  sendPrompt(prompt: string, timeout: number): Promise<AgentResult>;
  sendFollowup(prompt: string, timeout: number): Promise<AgentResult>;
  teardown(): Promise<void>;
  /** Update the working directory used for subsequent agent invocations. */
  setWorkingDirectory(dir: string): void;
}

export abstract class BaseAgentDriver implements AgentDriver {
  protected workspace: string = ".";
  protected bootstrapSkillSourceDir: string = "";

  /** Short tag used in log output (e.g. "claude-code", "amp") */
  protected abstract readonly driverName: string;

  /** Directories relative to workspace where the bootstrap skill should be copied. */
  protected abstract readonly skillDirs: string[];

  /** Returns the driver log tag, e.g. `claude-code` */
  protected get logPrefix(): string {
    return this.driverName;
  }

  async setup(workspace: string, bootstrapSkillSourceDir: string): Promise<void> {
    this.workspace = workspace;
    this.bootstrapSkillSourceDir = bootstrapSkillSourceDir;
    await this.seedBootstrapSkill();
  }

  setWorkingDirectory(dir: string): void {
    this.workspace = dir;
  }

  protected async seedBootstrapSkill(): Promise<void> {
    const sourceDir = path.resolve(this.bootstrapSkillSourceDir);
    await fs.access(path.join(sourceDir, "SKILL.md"));

    for (const targetDir of this.skillDirs) {
      const destDir = path.join(this.workspace, targetDir, "golem-new-project");
      await fs.mkdir(path.dirname(destDir), { recursive: true });
      await fs.cp(sourceDir, destDir, { recursive: true });
    }
  }

  abstract sendPrompt(prompt: string, timeout: number): Promise<AgentResult>;
  abstract sendFollowup(prompt: string, timeout: number): Promise<AgentResult>;
  abstract teardown(): Promise<void>;

  protected async runCommand(
    command: string,
    args: string[],
    timeoutSeconds: number,
    cwd?: string,
  ): Promise<AgentResult> {
    const startTime = Date.now();

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd: cwd || this.workspace,
        detached: true,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let output = "";
      let stdoutBuf = "";
      let stderrBuf = "";
      let timedOut = false;
      const prefix = this.logPrefix;

      child.stdout?.on("data", (data) => {
        const chunk = data.toString();
        output += chunk;
        stdoutBuf += chunk;
        const lines = stdoutBuf.split("\n");
        stdoutBuf = lines.pop()!;
        for (const line of lines) {
          log.driver(prefix, line);
        }
      });
      child.stderr?.on("data", (data) => {
        const chunk = data.toString();
        output += chunk;
        stderrBuf += chunk;
        const lines = stderrBuf.split("\n");
        stderrBuf = lines.pop()!;
        for (const line of lines) {
          log.driverErr(prefix, line);
        }
      });

      const timeoutId = setTimeout(() => {
        timedOut = true;
        killProcessTree(child);
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
        if (stdoutBuf) log.driver(prefix, stdoutBuf);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: !timedOut && exitCode === 0,
          output: timedOut ? `Timed out after ${timeoutSeconds}s. ${output}` : output,
          durationSeconds,
          exitCode: timedOut ? null : exitCode,
        });
      });

      child.on("error", (err) => {
        clearTimeout(timeoutId);
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: false,
          output: output + (err.message || "Unknown error"),
          durationSeconds,
          exitCode: null,
        });
      });
    });
  }
}
