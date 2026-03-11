import { spawn } from "node:child_process";

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

  async setup(workspace: string, skillsDir: string): Promise<void> {
    this.workspace = workspace;
    this.skillsDir = skillsDir;
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
      child.stdout?.on("data", (data) => {
        const chunk = data.toString();
        output += chunk;
        process.stdout.write(chunk);
      });
      child.stderr?.on("data", (data) => {
        const chunk = data.toString();
        output += chunk;
        process.stderr.write(chunk);
      });

      const timeoutId = setTimeout(() => {
        controller.abort();
      }, timeoutSeconds * 1000);

      child.on("close", (exitCode) => {
        clearTimeout(timeoutId);
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
