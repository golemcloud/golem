import {
  BaseAgentDriver,
  AgentResult,
  ActivityMonitor,
  type DriverTimeoutOptions,
} from "./base.js";
import { execute, type AmpOptions } from "@sourcegraph/amp-sdk";
import * as log from "../log.js";

const VALID_MODES = ["smart", "rush", "deep"] as const;
type ValidMode = (typeof VALID_MODES)[number];

export class AmpAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "amp";
  protected readonly skillDirs = [".agents/skills"];
  private sessionId: string | null = null;
  private activatedSkillNames: Set<string> = new Set();

  async sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    this.sessionId = null;
    return this.executeAmp(prompt, opts);
  }

  async sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult> {
    if (!this.sessionId) {
      return this.sendPrompt(prompt, opts);
    }
    return this.executeAmp(prompt, opts, this.sessionId);
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

  private async executeAmp(
    prompt: string,
    opts: DriverTimeoutOptions,
    continueSessionId?: string,
  ): Promise<AgentResult> {
    const startTime = Date.now();
    const outputParts: string[] = [];

    const options: AmpOptions = {
      cwd: this.workspace,
      dangerouslyAllowAll: true,
      visibility: "private",
      labels: ["golem-skill-harness"],
    };

    const modeEnv = process.env.AMP_MODE;
    if (modeEnv) {
      if ((VALID_MODES as readonly string[]).includes(modeEnv)) {
        options.mode = modeEnv as ValidMode;
      } else {
        return {
          success: false,
          output: `Invalid AMP_MODE "${modeEnv}". Must be one of: ${VALID_MODES.join(", ")}`,
          durationSeconds: 0,
          exitCode: 1,
        };
      }
    }

    if (continueSessionId) {
      options.continue = continueSessionId;
    }

    const prefix = this.logPrefix;

    const controller = new AbortController();
    const monitor = new ActivityMonitor(prefix, opts, (kind) => {
      controller.abort(new Error(`${kind}-timeout`));
    });
    const signal = controller.signal;

    try {
      for await (const message of execute({ prompt, options, signal })) {
        monitor.noteActivity();
        this.sessionId ??= message.session_id;

        if (message.type === "system" && message.subtype === "init") {
          log.driverSession(prefix, message.session_id);
          log.driverCwd(prefix, message.cwd);
          if (message.tools.length > 0) {
            log.driverTools(prefix, message.tools);
          }
          for (const mcp of message.mcp_servers) {
            log.driverMcp(prefix, mcp.name, mcp.status);
          }
        } else if (message.type === "assistant") {
          for (const block of message.message.content) {
            if (block.type === "text") {
              outputParts.push(block.text);
              for (const line of block.text.split("\n")) {
                log.driver(prefix, line);
              }
            } else if (block.type === "tool_use") {
              log.driverToolUse(
                prefix,
                block.name,
                block.input as Record<string, unknown> | undefined,
              );
              if (block.name === "skill") {
                const input = block.input as Record<string, unknown> | undefined;
                const skillName = typeof input?.name === "string" ? input.name : undefined;
                if (skillName) {
                  this.activatedSkillNames.add(skillName);
                }
              }
            }
          }
        } else if (message.type === "result") {
          const durationSeconds = (Date.now() - startTime) / 1000;
          const durationStr = `(${durationSeconds.toFixed(1)}s)`;

          if (message.is_error) {
            log.driverError(prefix, message.error || "", durationStr);
            return {
              success: false,
              output: message.error || outputParts.join("") || "Unknown Amp error",
              durationSeconds,
              exitCode: 1,
            };
          }

          log.driverSuccess(prefix, durationStr, `turns=${message.num_turns}`);
          return {
            success: true,
            output: message.result || outputParts.join(""),
            durationSeconds,
            exitCode: 0,
          };
        }
      }

      const durationSeconds = (Date.now() - startTime) / 1000;
      log.driverStreamEnd(prefix);
      return {
        success: false,
        output: outputParts.join("") || "Amp stream ended without a final result",
        durationSeconds,
        exitCode: 1,
      };
    } catch (err: unknown) {
      const durationSeconds = (Date.now() - startTime) / 1000;
      const errMsg = err instanceof Error ? err.message : String(err);

      if (/abort|timeout/i.test(errMsg)) {
        return {
          success: false,
          output: `${monitor.formatTimeoutMessage("Amp")}. ${outputParts.join("")}`,
          durationSeconds,
          exitCode: null,
          timedOut: true,
          timeoutKind: monitor.timeoutKind ?? "step",
        };
      }
      if (/command not found|ENOENT/i.test(errMsg)) {
        log.driverNotInstalled(prefix);
        return {
          success: false,
          output: `Amp CLI not installed. ${errMsg}`,
          durationSeconds,
          exitCode: null,
        };
      }
      if (/auth|api key|unauthorized/i.test(errMsg)) {
        log.driverAuthFailed(prefix);
        return {
          success: false,
          output: `Amp authentication failed. ${errMsg}`,
          durationSeconds,
          exitCode: null,
        };
      }

      log.driverFatal(prefix, errMsg);
      return {
        success: false,
        output: `Amp error: ${errMsg}`,
        durationSeconds,
        exitCode: null,
      };
    } finally {
      monitor.finish();
    }
  }
}
