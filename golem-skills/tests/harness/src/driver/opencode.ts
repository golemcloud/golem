import { spawn } from "node:child_process";
import {
  BaseAgentDriver,
  AgentResult,
  killProcessTree,
  ActivityMonitor,
  CREDIT_INSUFFICIENT_PATTERN,
  type DriverTimeoutOptions,
} from "./base.js";
import * as log from "../log.js";

const STARTUP_STALL_TIMEOUT_SECONDS = 120;

export class OpenCodeAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "opencode";
  protected readonly skillDirs = [".agents/skills"];
  private lastSessionId: string | null = null;
  private activatedSkillNames: Set<string> = new Set();

  private buildArgs(prompt: string, isFollowup: boolean): string[] {
    const args = ["run", "--format", "json", "--dangerously-skip-permissions"];
    const model = process.env.OPENCODE_MODEL;
    if (model) {
      args.push("-m", model);
    }
    if (isFollowup && this.lastSessionId) {
      args.push("--continue", "--session", this.lastSessionId);
    }
    args.push(prompt);
    return args;
  }

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    this.lastSessionId = null;
    return this.executeOpencode(prompt, false, opts);
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.executeOpencode(prompt, true, opts);
  }

  async teardown(): Promise<void> {
    this.lastSessionId = null;
    this.activatedSkillNames.clear();
  }

  getActivatedSkills(): string[] | undefined {
    return Array.from(this.activatedSkillNames);
  }

  resetActivatedSkills(): void {
    this.activatedSkillNames.clear();
  }

  private async executeOpencode(
    prompt: string,
    isFollowup: boolean,
    opts: DriverTimeoutOptions,
  ): Promise<AgentResult> {
    const prefix = this.logPrefix;
    const startTime = Date.now();
    const textParts: string[] = [];

    return new Promise((resolve) => {
      const args = this.buildArgs(prompt, isFollowup);
      const child = spawn("opencode", args, {
        cwd: this.workspace,
        detached: true,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let rawOutput = "";
      let stdoutBuf = "";
      let stderrBuf = "";
      let stdoutBytes = 0;
      let stderrBytes = 0;
      let receivedFirstOutput = false;
      let startupStalled = false;
      let creditInsufficient = false;
      let encounteredError = false;

      const model = process.env.OPENCODE_MODEL;
      log.driver(
        prefix,
        `spawn: cmd=opencode args=[${args.join(", ")}] cwd=${this.workspace} pid=${child.pid ?? "?"} ` +
          `followup=${isFollowup} lastSession=${this.lastSessionId ?? "none"} ` +
          `model=${model ?? "default"} ` +
          `ANTHROPIC_API_KEY=${process.env.ANTHROPIC_API_KEY ? "present" : "absent"} ` +
          `OPENAI_API_KEY=${process.env.OPENAI_API_KEY ? "present" : "absent"} ` +
          `OPENCODE_MODEL=${model ? "present" : "absent"}`,
      );

      // OpenCode in --format json mode writes nothing to stdout until the
      // entire run completes. Its BubbleTea spinner may or may not produce
      // stderr bytes depending on TTY detection. Disable idle timeout since
      // it's unreliable here — the step timeout provides the safety net.
      const monitor = new ActivityMonitor(prefix, { ...opts, idleTimeoutSeconds: 0 }, (_kind) => {
        killProcessTree(child);
      });

      // Startup stall timer — fires if neither stdout nor stderr produces
      // any output within the timeout. Uses a generous timeout since Gemini
      // models can take a long time before the first response chunk.
      const startupTimer = setTimeout(() => {
        if (!receivedFirstOutput) {
          startupStalled = true;
          log.driverFatal(
            prefix,
            `✗ agent startup stall — no output for ${STARTUP_STALL_TIMEOUT_SECONDS}s`,
          );
          killProcessTree(child);
        }
      }, STARTUP_STALL_TIMEOUT_SECONDS * 1000);

      child.stdout?.on("data", (data) => {
        monitor.noteActivity();
        const chunk = data.toString();
        stdoutBytes += data.length;
        rawOutput += chunk;
        stdoutBuf += chunk;

        if (!receivedFirstOutput) {
          receivedFirstOutput = true;
          clearTimeout(startupTimer);
        }

        const lines = stdoutBuf.split("\n");
        stdoutBuf = lines.pop()!;
        for (const line of lines) {
          const signal = this.processJsonLine(prefix, line, textParts);
          if (signal === "credit") {
            creditInsufficient = true;
            log.driverFatal(prefix, "✗ credit balance too low — aborting run");
            killProcessTree(child);
            return;
          }
          if (signal === "error") {
            encounteredError = true;
          }
        }
      });

      child.stderr?.on("data", (data) => {
        monitor.noteActivity();
        const chunk = data.toString();
        stderrBytes += data.length;
        rawOutput += chunk;
        stderrBuf += chunk;

        if (!receivedFirstOutput) {
          receivedFirstOutput = true;
          clearTimeout(startupTimer);
        }

        const lines = stderrBuf.split("\n");
        stderrBuf = lines.pop()!;
        for (const line of lines) {
          log.driverErr(prefix, line);
        }
      });

      child.on("close", (exitCode) => {
        clearTimeout(startupTimer);
        monitor.finish();
        if (stdoutBuf) {
          const signal = this.processJsonLine(prefix, stdoutBuf, textParts);
          if (signal === "credit") {
            creditInsufficient = true;
          } else if (signal === "error") {
            encounteredError = true;
          }
        }
        if (stderrBuf) log.driverErr(prefix, stderrBuf);

        const durationSeconds = (Date.now() - startTime) / 1000;
        const durationStr = `(${durationSeconds.toFixed(1)}s)`;

        if (creditInsufficient) {
          resolve({
            success: false,
            output: `Credit balance too low. ${rawOutput}`,
            durationSeconds,
            exitCode: exitCode,
            creditInsufficient: true,
          });
          return;
        }

        if (startupStalled) {
          log.driver(
            prefix,
            `startup stall exit: stdoutBytes=${stdoutBytes} stderrBytes=${stderrBytes}`,
          );
          resolve({
            success: false,
            output: `OpenCode startup stall — no output for ${STARTUP_STALL_TIMEOUT_SECONDS}s. ${rawOutput}`,
            durationSeconds,
            exitCode: null,
            timedOut: true,
            timeoutKind: "idle",
          });
          return;
        }

        if (monitor.isTimedOut) {
          log.driver(prefix, `timeout exit: stdoutBytes=${stdoutBytes} stderrBytes=${stderrBytes}`);
          resolve({
            success: false,
            output: `${monitor.formatTimeoutMessage("OpenCode")}. ${rawOutput}`,
            durationSeconds,
            exitCode: null,
            timedOut: true,
            timeoutKind: monitor.timeoutKind,
          });
          return;
        }

        const success = exitCode === 0 && !encounteredError;
        const output = textParts.join("\n") || rawOutput;

        if (!success) {
          const msg = rawOutput || "Unknown error";
          if (/command not found|ENOENT/i.test(msg)) {
            log.driverFatal(prefix, "opencode CLI not installed");
          } else if (/\bauthentication\s+failed\b|invalid.*api.key|unauthorized/i.test(msg)) {
            log.driverAuthFailed(prefix);
          } else {
            log.driverError(prefix, msg, durationStr);
          }
        } else {
          log.driverSuccess(prefix, durationStr);
        }

        resolve({ success, output, durationSeconds, exitCode });
      });

      child.on("error", (err) => {
        clearTimeout(startupTimer);
        monitor.finish();
        const durationSeconds = (Date.now() - startTime) / 1000;
        log.driverFatal(prefix, err.message || "Unknown error");
        resolve({
          success: false,
          output: rawOutput + (err.message || "Unknown error"),
          durationSeconds,
          exitCode: null,
        });
      });
    });
  }

  /**
   * Parse a single JSONL line from opencode's `--format json` output and emit
   * structured log output. Returns `"credit"` if a credit-insufficient error
   * was detected, `"error"` for other error events, or `null` otherwise.
   */
  private processJsonLine(
    prefix: string,
    line: string,
    textParts: string[],
  ): "credit" | "error" | null {
    const trimmed = line.trim();
    if (!trimmed) return null;

    let event: Record<string, unknown>;
    try {
      event = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      // Not JSON — log as plain text
      log.driver(prefix, trimmed);
      return null;
    }

    const sessionId = event.sessionID as string | undefined;
    if (sessionId && !this.lastSessionId) {
      this.lastSessionId = sessionId;
      log.driverSession(prefix, sessionId);
    }

    const type = event.type as string | undefined;
    const part = event.part as Record<string, unknown> | undefined;

    switch (type) {
      case "step_start": {
        // Logged session above; nothing else needed
        break;
      }

      case "tool_use": {
        if (part) {
          const toolName = (part.tool as string) || "unknown";
          const state = part.state as Record<string, unknown> | undefined;
          const input = state?.input as Record<string, unknown> | undefined;
          log.driverToolUse(prefix, toolName, input);
          if (toolName === "skill") {
            const skillName = typeof input?.name === "string" ? input.name : undefined;
            if (skillName) {
              this.activatedSkillNames.add(skillName);
            }
          }
        }
        break;
      }

      case "text": {
        if (part) {
          const text = part.text as string | undefined;
          if (text) {
            textParts.push(text);
            for (const l of text.split("\n")) {
              log.driver(prefix, l);
            }
            if (CREDIT_INSUFFICIENT_PATTERN.test(text)) return "credit";
          }
        }
        break;
      }

      case "step_finish": {
        if (part) {
          const tokens = part.tokens as Record<string, unknown> | undefined;
          const cost = part.cost as number | undefined;
          if (tokens || cost !== undefined) {
            const input = (tokens?.input as number) ?? 0;
            const output = (tokens?.output as number) ?? 0;
            const extra = `tokens=${input}+${output}` + (cost ? ` cost=$${cost.toFixed(4)}` : "");
            log.driver(prefix, extra);
          }
        }
        break;
      }

      case "error": {
        const error = event.error as Record<string, unknown> | undefined;
        const errData = error?.data as Record<string, unknown> | undefined;
        const errMsg = (errData?.message as string) || (error?.name as string) || "unknown error";
        log.driverError(prefix, errMsg);
        if (CREDIT_INSUFFICIENT_PATTERN.test(errMsg)) return "credit";
        return "error";
      }

      default: {
        // Unknown event type — log raw for debugging
        log.driver(prefix, trimmed);
        break;
      }
    }

    return null;
  }
}
