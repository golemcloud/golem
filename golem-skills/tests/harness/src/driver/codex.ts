import { spawn } from "node:child_process";
import { BaseAgentDriver, AgentResult, killProcessTree, ActivityMonitor, type DriverTimeoutOptions } from "./base.js";
import * as log from "../log.js";

export class CodexAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "codex";
  protected readonly skillDirs = [".agents/skills"];
  private lastSessionId: string | null = null;

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    const result = await this.executeCodex(
      ["exec", "--dangerously-bypass-approvals-and-sandbox", "--json", prompt],
      opts,
    );
    if (!result.success && result.output.includes("command not found")) {
      result.output = `Codex CLI not installed. ${result.output}`;
    }
    if (!result.success && /auth|api key|unauthorized/i.test(result.output)) {
      result.output = `Codex authentication failed. ${result.output}`;
    }
    return result;
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    if (!this.lastSessionId) {
      return this.sendPrompt(prompt, opts);
    }
    return this.executeCodex(
      [
        "exec",
        "resume",
        this.lastSessionId,
        "--dangerously-bypass-approvals-and-sandbox",
        "--json",
        prompt,
      ],
      opts,
    );
  }

  async teardown(): Promise<void> {
    this.lastSessionId = null;
  }

  private executeCodex(args: string[], opts: DriverTimeoutOptions): Promise<AgentResult> {
    const startTime = Date.now();
    const prefix = this.logPrefix;

    return new Promise((resolve) => {
      const child = spawn("codex", args, {
        cwd: this.workspace,
        detached: true,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      const monitor = new ActivityMonitor(prefix, opts, (_kind) => {
        killProcessTree(child);
      });

      const outputParts: string[] = [];
      let stdoutBuf = "";
      let stderrBuf = "";

      child.stdout?.on("data", (data: Buffer) => {
        monitor.noteActivity();
        stdoutBuf += data.toString();
        const lines = stdoutBuf.split("\n");
        stdoutBuf = lines.pop()!;
        for (const line of lines) {
          this.handleJsonLine(prefix, line, outputParts, startTime);
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
        if (stdoutBuf) this.handleJsonLine(prefix, stdoutBuf, outputParts, startTime);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: !monitor.isTimedOut && exitCode === 0,
          output: monitor.isTimedOut
            ? `${monitor.formatTimeoutMessage("Codex")}. ${outputParts.join("")}`
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
        resolve({
          success: false,
          output: outputParts.join("") + (err.message || "Unknown error"),
          durationSeconds,
          exitCode: null,
        });
      });
    });
  }

  private handleJsonLine(
    prefix: string,
    line: string,
    outputParts: string[],
    startTime: number,
  ): void {
    const trimmed = line.trim();
    if (!trimmed) return;

    let event: Record<string, unknown>;
    try {
      event = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      log.driver(prefix, line);
      return;
    }

    const type = event.type as string | undefined;
    if (!type) {
      log.driver(prefix, line);
      return;
    }

    switch (type) {
      case "thread.started": {
        const threadId = event.thread_id as string | undefined;
        if (threadId) {
          this.lastSessionId = threadId;
          log.driverSession(prefix, threadId);
        }
        break;
      }

      case "turn.started":
        break;

      case "turn.completed": {
        const durationStr = `(${((Date.now() - startTime) / 1000).toFixed(1)}s)`;
        const usage = event.usage as Record<string, unknown> | undefined;
        let extra: string | undefined;
        if (usage) {
          const input = usage.input_tokens as number | undefined;
          const output = usage.output_tokens as number | undefined;
          if (input != null || output != null) {
            extra = `tokens=${input ?? 0}in/${output ?? 0}out`;
          }
        }
        log.driverSuccess(prefix, durationStr, extra);
        break;
      }

      case "turn.failed": {
        const error = event.error as Record<string, unknown> | undefined;
        const msg = (error?.message as string) ?? "unknown error";
        log.driverError(prefix, msg);
        break;
      }

      case "item.started":
      case "item.updated":
      case "item.completed": {
        const item = event.item as Record<string, unknown> | undefined;
        if (!item) break;
        this.handleItem(prefix, item, type, outputParts);
        break;
      }

      case "error": {
        const msg = (event.message as string) ?? "fatal stream error";
        log.driverError(prefix, msg);
        break;
      }

      default:
        log.driver(prefix, line);
        break;
    }
  }

  private handleItem(
    prefix: string,
    item: Record<string, unknown>,
    eventType: string,
    outputParts: string[],
  ): void {
    const itemType = item.type as string | undefined;
    if (!itemType) return;

    switch (itemType) {
      case "agent_message": {
        const text = item.text as string | undefined;
        if (text) {
          outputParts.push(text);
          for (const line of text.split("\n")) {
            log.driver(prefix, line);
          }
        }
        break;
      }

      case "command_execution": {
        const command = item.command as string | undefined;
        if (eventType === "item.started" && command) {
          log.driverToolUse(prefix, "bash", { command });
        }
        if (eventType === "item.completed") {
          const exitCode = item.exit_code as number | undefined;
          const output = item.aggregated_output as string | undefined;
          if (output) {
            for (const line of output.split("\n").slice(0, 10)) {
              log.driverErr(prefix, line);
            }
          }
          if (exitCode != null && exitCode !== 0) {
            log.driverError(prefix, `command exited with code ${exitCode}`);
          }
        }
        break;
      }

      case "file_change": {
        const changes = item.changes as Array<Record<string, unknown>> | undefined;
        if (changes) {
          for (const change of changes) {
            const kind = (change.kind as string) ?? "update";
            const path = (change.path as string) ?? "?";
            log.driver(prefix, `file: ${kind} ${path}`);
          }
        }
        break;
      }

      case "mcp_tool_call": {
        if (eventType === "item.started" || eventType === "item.completed") {
          const server = item.server as string | undefined;
          const tool = item.tool as string | undefined;
          const args = item.arguments as Record<string, unknown> | undefined;
          if (server && tool) {
            log.driverToolUse(prefix, `mcp:${server}.${tool}`, args);
          }
        }
        break;
      }

      case "reasoning": {
        const text = item.text as string | undefined;
        if (text) {
          for (const line of text.split("\n")) {
            log.driver(prefix, line);
          }
        }
        break;
      }

      case "todo_list": {
        const items = item.items as Array<Record<string, unknown>> | undefined;
        if (items) {
          for (const todo of items) {
            const completed = todo.completed as boolean;
            const text = (todo.text as string) ?? "";
            log.driver(prefix, `${completed ? "✓" : "○"} ${text}`);
          }
        }
        break;
      }

      case "error": {
        const message = (item.message as string) ?? "unknown item error";
        log.driverError(prefix, message);
        break;
      }

      default:
        break;
    }
  }
}
