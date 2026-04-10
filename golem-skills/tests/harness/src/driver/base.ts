import { spawn } from "node:child_process";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import chalk from "chalk";
import * as log from "../log.js";

export interface AgentResult {
  success: boolean;
  output: string;
  durationSeconds: number;
  exitCode: number | null;
}

export interface AgentDriver {
  setup(workspace: string, skillsDir: string): Promise<void>;
  sendPrompt(prompt: string, timeout: number): Promise<AgentResult>;
  sendFollowup(prompt: string, timeout: number): Promise<AgentResult>;
  teardown(): Promise<void>;
}

export abstract class BaseAgentDriver implements AgentDriver {
  protected workspace: string = ".";
  protected skillsDir: string = "";
  protected readonly skillLinkMode: "symlink" | "copy" = "symlink";

  /** Short name used as a colored prefix in log output (e.g. "claude", "amp") */
  protected abstract readonly driverName: string;

  /** Directories relative to workspace where skills should be symlinked (e.g. ['.claude/skills']) */
  protected abstract readonly skillDirs: string[];

  /** Returns the colored log prefix for this driver, e.g. `[claude]` */
  protected get logPrefix(): string {
    return chalk.magenta(`[${this.driverName}]`);
  }

  async setup(workspace: string, skillsDir: string): Promise<void> {
    this.workspace = workspace;
    this.skillsDir = skillsDir;
    await this.linkSkills();
  }

  protected async linkSkills(): Promise<void> {
    const skills = await fs.readdir(this.skillsDir, { withFileTypes: true });
    for (const targetDir of this.skillDirs) {
      const absTargetDir = path.join(this.workspace, targetDir);
      await fs.mkdir(absTargetDir, { recursive: true });

      for (const dirent of skills) {
        if (!dirent.isDirectory()) continue;
        const sourceFile = path.join(
          path.resolve(this.skillsDir, dirent.name),
          "SKILL.md",
        );
        try {
          await fs.access(sourceFile);
        } catch {
          continue;
        }
        const destDir = path.join(absTargetDir, dirent.name);
        await fs.mkdir(destDir, { recursive: true });
        const destFile = path.join(destDir, "SKILL.md");
        if (this.skillLinkMode === "copy") {
          await fs.copyFile(sourceFile, destFile).catch(() => {});
        } else {
          await fs.symlink(sourceFile, destFile).catch(() => {});
        }
      }
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
    const controller = new AbortController();
    const { signal } = controller;

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd: cwd || this.workspace,
        signal,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let output = "";
      let stdoutBuf = "";
      let stderrBuf = "";
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
        controller.abort();
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
        if (stdoutBuf) log.driver(prefix, stdoutBuf);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: exitCode === 0,
          output,
          durationSeconds,
          exitCode,
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
