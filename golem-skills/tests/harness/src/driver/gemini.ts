import { spawn } from "node:child_process";
import {
  BaseAgentDriver,
  AgentResult,
  killProcessTree,
  ActivityMonitor,
  type DriverTimeoutOptions,
} from "./base.js";
import * as log from "../log.js";

const STARTUP_STALL_TIMEOUT_SECONDS = 90;

export class GeminiAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "gemini";
  protected readonly skillDirs = [".gemini/skills"];
  private lastSessionIndex: string | null = null;

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    this.lastSessionIndex = null;
    return this.executeGemini(prompt, false, opts);
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.executeGemini(prompt, true, opts);
  }

  async teardown(): Promise<void> {
    this.lastSessionIndex = null;
  }

  private buildArgs(prompt: string, isFollowup: boolean): string[] {
    const args = ["--approval-mode", "yolo", "--output-format", "stream-json", "-p", prompt];

    if (isFollowup && this.lastSessionIndex) {
      args.push("--resume", this.lastSessionIndex);
    }

    return args;
  }

  private async executeGemini(
    prompt: string,
    isFollowup: boolean,
    opts: DriverTimeoutOptions,
  ): Promise<AgentResult> {
    const prefix = this.logPrefix;
    const startTime = Date.now();
    const textParts: string[] = [];

    return new Promise((resolve) => {
      const args = this.buildArgs(prompt, isFollowup);
      const child = spawn("gemini", args, {
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

      log.driver(
        prefix,
        `spawn: cmd=gemini args=[${args.join(", ")}] cwd=${this.workspace} pid=${child.pid ?? "?"} ` +
          `followup=${isFollowup} lastSession=${this.lastSessionIndex ?? "none"} ` +
          `GOOGLE_GENERATIVE_AI_API_KEY=${process.env.GOOGLE_GENERATIVE_AI_API_KEY ? "present" : "absent"} ` +
          `GEMINI_API_KEY=${process.env.GEMINI_API_KEY ? "present" : "absent"}`,
      );

      const monitor = new ActivityMonitor(prefix, opts, (_kind) => {
        killProcessTree(child);
      });

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
          this.processJsonLine(prefix, line, textParts);
        }
      });

      child.stderr?.on("data", (data) => {
        monitor.noteActivity();
        const chunk = data.toString();
        stderrBytes += data.length;
        rawOutput += chunk;
        stderrBuf += chunk;

        const lines = stderrBuf.split("\n");
        stderrBuf = lines.pop()!;
        for (const line of lines) {
          log.driverErr(prefix, line);
        }
      });

      child.on("close", (exitCode) => {
        clearTimeout(startupTimer);
        monitor.finish();
        if (stdoutBuf) this.processJsonLine(prefix, stdoutBuf, textParts);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);

        const durationSeconds = (Date.now() - startTime) / 1000;
        const durationStr = `(${durationSeconds.toFixed(1)}s)`;

        if (startupStalled) {
          log.driver(
            prefix,
            `startup stall exit: stdoutBytes=${stdoutBytes} stderrBytes=${stderrBytes}`,
          );
          resolve({
            success: false,
            output: `Gemini startup stall — no output for ${STARTUP_STALL_TIMEOUT_SECONDS}s. ${rawOutput}`,
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
            output: `${monitor.formatTimeoutMessage("Gemini")}. ${rawOutput}`,
            durationSeconds,
            exitCode: null,
            timedOut: true,
            timeoutKind: monitor.timeoutKind,
          });
          return;
        }

        const success = exitCode === 0;
        const output = textParts.join("\n") || rawOutput;

        if (!success) {
          const msg = rawOutput || "Unknown error";
          if (/command not found|ENOENT/i.test(msg)) {
            log.driverFatal(prefix, "gemini CLI not installed");
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
   * Parse a single stream-json line from gemini's `--output-format stream-json` output.
   *
   * Gemini stream-json emits objects with a `type` field. Known types:
   *   system/init  — session info
   *   assistant    — text output from model
   *   tool_use     — tool invocation
   *   result       — final result
   */
  private processJsonLine(prefix: string, line: string, textParts: string[]): void {
    const trimmed = line.trim();
    if (!trimmed) return;

    let event: Record<string, unknown>;
    try {
      event = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      log.driver(prefix, trimmed);
      return;
    }

    const type = event.type as string | undefined;

    switch (type) {
      case "system":
      case "init": {
        const sessionId = (event.sessionId ?? event.session_id ?? event.sessionIndex) as
          | string
          | undefined;
        if (sessionId && !this.lastSessionIndex) {
          this.lastSessionIndex = sessionId;
          log.driverSession(prefix, sessionId);
        }
        break;
      }

      case "assistant":
      case "text": {
        const text =
          (event.text as string | undefined) ??
          (event.content as string | undefined) ??
          ((event.message as Record<string, unknown>)?.content as string | undefined);
        if (text) {
          textParts.push(text);
          for (const l of text.split("\n")) {
            log.driver(prefix, l);
          }
        }
        break;
      }

      case "tool_use":
      case "tool_call": {
        const toolName =
          (event.tool as string | undefined) ?? (event.name as string | undefined) ?? "unknown";
        const input = event.input as Record<string, unknown> | undefined;
        log.driverToolUse(prefix, toolName, input);
        break;
      }

      case "result": {
        const text = (event.text as string | undefined) ?? (event.content as string | undefined);
        if (text) {
          textParts.push(text);
          for (const l of text.split("\n")) {
            log.driver(prefix, l);
          }
        }
        break;
      }

      case "error": {
        const errMsg =
          (event.message as string | undefined) ??
          (event.error as string | undefined) ??
          "unknown error";
        log.driverError(prefix, errMsg);
        break;
      }

      default: {
        log.driver(prefix, trimmed);
        break;
      }
    }
  }
}
