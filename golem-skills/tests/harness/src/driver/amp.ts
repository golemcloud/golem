import { BaseAgentDriver, AgentResult } from "./base.js";
import { execute, type AmpOptions } from "@sourcegraph/amp-sdk";
import chalk from "chalk";

const VALID_MODES = ["smart", "rush", "deep"] as const;
type ValidMode = (typeof VALID_MODES)[number];

export class AmpAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "amp";
  protected readonly skillDirs = [".agents/skills"];
  private sessionId: string | null = null;

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    this.sessionId = null;
    return this.executeAmp(prompt, timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    if (!this.sessionId) {
      return this.sendPrompt(prompt, timeout);
    }
    return this.executeAmp(prompt, timeout, this.sessionId);
  }

  async teardown(): Promise<void> {
    this.sessionId = null;
  }

  private async executeAmp(
    prompt: string,
    timeoutSeconds: number,
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

    try {
      const signal = AbortSignal.timeout(timeoutSeconds * 1000);

      for await (const message of execute({ prompt, options, signal })) {
        this.sessionId ??= message.session_id;

        if (message.type === "system" && message.subtype === "init") {
          console.log(
            `${prefix} ${chalk.cyan("session")} ${chalk.gray(message.session_id)}`,
          );
          console.log(
            `${prefix} ${chalk.cyan("cwd")} ${chalk.gray(message.cwd)}`,
          );
          if (message.tools.length > 0) {
            console.log(
              `${prefix} ${chalk.cyan("tools")} ${chalk.gray(`(${message.tools.length})`)} ${chalk.gray(message.tools.slice(0, 8).join(", "))}${message.tools.length > 8 ? chalk.gray(", ...") : ""}`,
            );
          }
          for (const mcp of message.mcp_servers) {
            const statusColor =
              mcp.status === "connected" ? chalk.green : chalk.yellow;
            console.log(
              `${prefix} ${chalk.cyan("mcp")} ${chalk.white(mcp.name)} ${statusColor(mcp.status)}`,
            );
          }
        } else if (message.type === "assistant") {
          for (const block of message.message.content) {
            if (block.type === "text") {
              outputParts.push(block.text);
              for (const line of block.text.split("\n")) {
                console.log(`${prefix} ${line}`);
              }
            } else if (block.type === "tool_use") {
              const inputStr =
                block.input && Object.keys(block.input).length > 0
                  ? " " + chalk.gray(JSON.stringify(block.input))
                  : "";
              console.log(
                `${prefix} ${chalk.yellow("▶")} ${chalk.yellow(block.name)}${inputStr}`,
              );
            }
          }
        } else if (message.type === "result") {
          const durationSeconds = (Date.now() - startTime) / 1000;
          const durationStr = chalk.gray(`(${durationSeconds.toFixed(1)}s)`);

          if (message.is_error) {
            console.log(
              `${prefix} ${chalk.red("✗ error")} ${durationStr}`,
            );
            console.log(
              `${prefix} ${chalk.red(message.error)}`,
            );
            return {
              success: false,
              output:
                message.error || outputParts.join("") || "Unknown Amp error",
              durationSeconds,
              exitCode: 1,
            };
          }

          console.log(
            `${prefix} ${chalk.green("✓ done")} ${durationStr} ${chalk.gray(`turns=${message.num_turns}`)}`,
          );
          return {
            success: true,
            output: message.result || outputParts.join(""),
            durationSeconds,
            exitCode: 0,
          };
        }
      }

      const durationSeconds = (Date.now() - startTime) / 1000;
      console.log(
        `${prefix} ${chalk.red("✗ stream ended without result")}`,
      );
      return {
        success: false,
        output:
          outputParts.join("") || "Amp stream ended without a final result",
        durationSeconds,
        exitCode: 1,
      };
    } catch (err: unknown) {
      const durationSeconds = (Date.now() - startTime) / 1000;
      const errMsg = err instanceof Error ? err.message : String(err);

      if (/abort|timeout/i.test(errMsg)) {
        console.log(
          `${prefix} ${chalk.red(`✗ timed out after ${timeoutSeconds}s`)}`,
        );
        return {
          success: false,
          output: `Amp timed out after ${timeoutSeconds}s. ${outputParts.join("")}`,
          durationSeconds,
          exitCode: null,
        };
      }
      if (/command not found|ENOENT/i.test(errMsg)) {
        console.log(
          `${prefix} ${chalk.red("✗ Amp CLI not installed")}`,
        );
        return {
          success: false,
          output: `Amp CLI not installed. ${errMsg}`,
          durationSeconds,
          exitCode: null,
        };
      }
      if (/auth|api key|unauthorized/i.test(errMsg)) {
        console.log(
          `${prefix} ${chalk.red("✗ authentication failed")}`,
        );
        return {
          success: false,
          output: `Amp authentication failed. ${errMsg}`,
          durationSeconds,
          exitCode: null,
        };
      }

      console.log(`${prefix} ${chalk.red(`✗ ${errMsg}`)}`);
      return {
        success: false,
        output: `Amp error: ${errMsg}`,
        durationSeconds,
        exitCode: null,
      };
    }
  }
}
