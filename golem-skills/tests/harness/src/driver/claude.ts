import { spawn } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import {
  BaseAgentDriver,
  AgentResult,
  killProcessTree,
  ActivityMonitor,
  type DriverTimeoutOptions,
} from "./base.js";
import * as log from "../log.js";

export class ClaudeAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "claude";
  protected readonly skillDirs = [".agents/skills"];
  private sessionId: string | null = null;
  private activatedSkillNames: Set<string> = new Set();

  async setup(workspace: string, bootstrapSkillSourceDirs: string[]): Promise<void> {
    await super.setup(workspace, bootstrapSkillSourceDirs);
    const agentsDir = path.join(workspace, ".agents");
    const claudeLink = path.join(workspace, ".claude");
    try {
      await fs.access(agentsDir);
      try {
        await fs.lstat(claudeLink);
      } catch {
        await fs.symlink(".agents", claudeLink);
      }
    } catch {
      // .agents doesn't exist, skip symlink
    }
  }

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    this.sessionId = null;
    return this.runClaudeStreamJson(
      [
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        prompt,
      ],
      opts,
    );
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    if (!this.sessionId) {
      return this.sendPrompt(prompt, opts);
    }
    return this.runClaudeStreamJson(
      [
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--resume",
        this.sessionId,
        prompt,
      ],
      opts,
    );
  }

  async teardown(): Promise<void> {
    this.sessionId = null;
    this.activatedSkillNames.clear();
  }

  getActivatedSkills(): string[] | undefined {
    return Array.from(this.activatedSkillNames);
  }

  resetActivatedSkills(): void {
    this.activatedSkillNames.clear();
  }

  private async runClaudeStreamJson(
    args: string[],
    opts: DriverTimeoutOptions,
  ): Promise<AgentResult> {
    const startTime = Date.now();
    const prefix = this.logPrefix;
    const outputParts: string[] = [];

    return new Promise((resolve) => {
      const child = spawn("claude", args, {
        cwd: this.workspace,
        detached: true,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      const monitor = new ActivityMonitor(prefix, opts, (_kind) => {
        killProcessTree(child);
      });

      let stdoutBuf = "";
      let stderrBuf = "";

      const processLine = (line: string): void => {
        if (!line.trim()) return;
        let msg: Record<string, unknown>;
        try {
          msg = JSON.parse(line);
        } catch {
          log.driver(prefix, line);
          return;
        }

        if (msg.type === "system" && msg.subtype === "init") {
          if (typeof msg.session_id === "string") {
            this.sessionId = msg.session_id;
            log.driverSession(prefix, msg.session_id);
          }
          if (typeof msg.cwd === "string") {
            log.driverCwd(prefix, msg.cwd);
          }
          if (Array.isArray(msg.tools) && msg.tools.length > 0) {
            log.driverTools(prefix, msg.tools as string[]);
          }
          if (Array.isArray(msg.mcp_servers)) {
            for (const mcp of msg.mcp_servers as { name: string; status: string }[]) {
              log.driverMcp(prefix, mcp.name, mcp.status);
            }
          }
        } else if (msg.type === "assistant") {
          const message = msg.message as { content?: unknown[] } | undefined;
          if (message && Array.isArray(message.content)) {
            for (const block of message.content as Record<string, unknown>[]) {
              if (block.type === "text" && typeof block.text === "string") {
                outputParts.push(block.text);
                for (const textLine of block.text.split("\n")) {
                  log.driver(prefix, textLine);
                }
              } else if (block.type === "tool_use") {
                log.driverToolUse(
                  prefix,
                  block.name as string,
                  block.input as Record<string, unknown> | undefined,
                );
                if (block.name === "Skill") {
                  const input = block.input as Record<string, unknown> | undefined;
                  const skillName = typeof input?.skill === "string" ? input.skill : undefined;
                  if (skillName) {
                    this.activatedSkillNames.add(skillName);
                  }
                }
              }
            }
          }
        } else if (msg.type === "result") {
          const durationSeconds = (Date.now() - startTime) / 1000;
          const durationStr = `(${durationSeconds.toFixed(1)}s)`;

          if (typeof msg.session_id === "string" && msg.session_id.length > 0) {
            this.sessionId = msg.session_id;
          }

          if (msg.subtype === "success" && msg.is_error === false) {
            const extra = typeof msg.num_turns === "number" ? `turns=${msg.num_turns}` : undefined;
            if (typeof msg.total_cost_usd === "number") {
              const costStr = `cost=$${(msg.total_cost_usd as number).toFixed(4)}`;
              log.driverSuccess(prefix, durationStr, extra ? `${extra} ${costStr}` : costStr);
            } else {
              log.driverSuccess(prefix, durationStr, extra);
            }
          } else {
            const errors = Array.isArray(msg.errors) ? (msg.errors as string[]).join("; ") : "";
            log.driverError(prefix, errors, durationStr);
          }
        } else if (msg.type === "tool_progress") {
          // Silently ignore progress updates
        } else if (msg.type === "system" && msg.subtype === "api_retry") {
          const attempt = msg.attempt ?? "?";
          const error = typeof msg.error === "string" ? msg.error : "";
          log.driverErr(prefix, `API retry attempt=${attempt} ${error}`);
        }
      };

      child.stdout?.on("data", (data: Buffer) => {
        monitor.noteActivity();
        stdoutBuf += data.toString();
        const lines = stdoutBuf.split("\n");
        stdoutBuf = lines.pop()!;
        for (const line of lines) {
          processLine(line);
        }
      });

      child.stderr?.on("data", (data: Buffer) => {
        monitor.noteActivity();
        stderrBuf += data.toString();
        const lines = stderrBuf.split("\n");
        stderrBuf = lines.pop()!;
        for (const line of lines) {
          log.driverErr(prefix, line);
        }
      });

      child.on("close", (exitCode) => {
        monitor.finish();
        if (stdoutBuf) processLine(stdoutBuf);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: !monitor.isTimedOut && exitCode === 0,
          output: monitor.isTimedOut
            ? `${monitor.formatTimeoutMessage("Claude")}. ${outputParts.join("")}`
            : outputParts.join(""),
          durationSeconds,
          exitCode: monitor.isTimedOut ? null : exitCode,
          timedOut: monitor.isTimedOut || undefined,
          timeoutKind: monitor.timeoutKind,
        });
      });

      child.on("error", (err) => {
        monitor.finish();
        const durationSeconds = (Date.now() - startTime) / 1000;
        const errMsg = err.message || "Unknown error";
        log.driverFatal(prefix, errMsg);
        resolve({
          success: false,
          output: outputParts.join("") + errMsg,
          durationSeconds,
          exitCode: null,
        });
      });
    });
  }
}
