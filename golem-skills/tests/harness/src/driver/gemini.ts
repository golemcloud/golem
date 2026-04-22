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

export class GeminiAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "gemini";
  protected readonly skillDirs = [".agents/skills"];
  // Gemini CLI has long silent gaps due to retry backoff (up to 300s on 429)
  // and extra LLM calls (next-speaker check, context compression).
  protected readonly defaultIdleTimeoutOverride = 600;
  private lastSessionId: string | null = null;
  private activatedSkillNames: Set<string> = new Set();

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    this.lastSessionId = null;
    return this.executeGemini(prompt, false, opts);
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.executeGemini(prompt, true, opts);
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

  private buildArgs(prompt: string, isFollowup: boolean): string[] {
    const args = ["--prompt", prompt, "--output-format", "stream-json", "--yolo"];
    if (isFollowup && this.lastSessionId) {
      args.push("--resume", this.lastSessionId);
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
      let creditInsufficient = false;

      log.driver(
        prefix,
        `spawn: cmd=gemini args=[${args.join(", ")}] cwd=${this.workspace} pid=${child.pid ?? "?"} ` +
          `followup=${isFollowup} lastSession=${this.lastSessionId ?? "none"} ` +
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
          if (this.processJsonLine(prefix, line, textParts)) {
            creditInsufficient = true;
            log.driverFatal(prefix, "✗ credit balance too low — aborting run");
            killProcessTree(child);
            return;
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
          if (this.processJsonLine(prefix, stdoutBuf, textParts)) {
            creditInsufficient = true;
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
   * Parse a single JSONL line from gemini's `--output-format stream-json`
   * output and emit structured log output. Returns `true` if a
   * credit-insufficient error was detected and the caller should abort.
   *
   * Gemini stream-json events:
   *   init        — session start (session_id, model)
   *   message     — user/assistant text (role, content, delta)
   *   tool_use    — tool call request (tool_name, parameters)
   *   tool_result — tool execution result (tool_id, status, output)
   *   error       — non-fatal error (severity, message)
   *   result      — session end (status, stats)
   */
  private processJsonLine(prefix: string, line: string, textParts: string[]): boolean {
    const trimmed = line.trim();
    if (!trimmed) return false;

    let event: Record<string, unknown>;
    try {
      event = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      log.driver(prefix, trimmed);
      return false;
    }

    const type = event.type as string | undefined;

    switch (type) {
      case "init": {
        const sessionId = event.session_id as string | undefined;
        if (sessionId) {
          this.lastSessionId = sessionId;
          log.driverSession(prefix, sessionId);
        }
        const model = event.model as string | undefined;
        if (model) {
          log.driver(prefix, `model=${model}`);
        }
        break;
      }

      case "message": {
        const role = event.role as string | undefined;
        const content = event.content as string | undefined;
        if (role === "assistant" && content) {
          textParts.push(content);
          for (const l of content.split("\n")) {
            log.driver(prefix, l);
          }
          if (CREDIT_INSUFFICIENT_PATTERN.test(content)) return true;
        }
        break;
      }

      case "tool_use": {
        const toolName = (event.tool_name as string) || "unknown";
        const parameters = event.parameters as Record<string, unknown> | undefined;
        log.driverToolUse(prefix, toolName, parameters);
        if (toolName === "activate_skill") {
          const skillName = typeof parameters?.name === "string" ? parameters.name : undefined;
          if (skillName) {
            this.activatedSkillNames.add(skillName);
          }
        }
        break;
      }

      case "tool_result": {
        const status = event.status as string | undefined;
        const error = event.error as Record<string, unknown> | undefined;
        if (status === "error" && error) {
          const errMsg = (error.message as string) || "tool error";
          log.driverError(prefix, errMsg);
          if (CREDIT_INSUFFICIENT_PATTERN.test(errMsg)) return true;
        }
        break;
      }

      case "error": {
        const message = (event.message as string) || "unknown error";
        const severity = event.severity as string | undefined;
        if (severity === "warning") {
          log.driverErr(prefix, `warning: ${message}`);
        } else {
          log.driverError(prefix, message);
        }
        if (CREDIT_INSUFFICIENT_PATTERN.test(message)) return true;
        break;
      }

      case "result": {
        const status = event.status as string | undefined;
        const stats = event.stats as Record<string, unknown> | undefined;
        if (stats) {
          const inputTokens = (stats.input_tokens as number) ?? 0;
          const outputTokens = (stats.output_tokens as number) ?? 0;
          const toolCalls = stats.tool_calls as number | undefined;
          let extra = `tokens=${inputTokens}+${outputTokens}`;
          if (toolCalls != null) extra += ` tools=${toolCalls}`;
          log.driver(prefix, extra);
        }
        if (status === "error") {
          const error = event.error as Record<string, unknown> | undefined;
          const errMsg = (error?.message as string) || "session error";
          log.driverError(prefix, errMsg);
          if (CREDIT_INSUFFICIENT_PATTERN.test(errMsg)) return true;
        }
        break;
      }

      default: {
        log.driver(prefix, trimmed);
        break;
      }
    }

    return false;
  }
}
