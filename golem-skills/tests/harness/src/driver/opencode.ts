import { BaseAgentDriver, AgentResult } from "./base.js";
import * as log from "../log.js";

export class OpenCodeAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "opencode";
  protected readonly skillDirs = [".agents/skills"];

  private buildArgs(prompt: string): string[] {
    const args = ["-p", prompt, "-f", "json", "-q"];
    const model = process.env.OPENCODE_MODEL;
    if (model) {
      args.push("-m", model);
    }
    return args;
  }

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    const prefix = this.logPrefix;
    const result = await this.runCommand("opencode", this.buildArgs(prompt), timeout);
    const durationStr = `(${result.durationSeconds.toFixed(1)}s)`;

    if (!result.success) {
      const msg = result.output || "Unknown error";
      if (/command not found|ENOENT/i.test(msg)) {
        log.driverFatal(prefix, "opencode CLI not installed");
      } else if (/auth|api key|unauthorized/i.test(msg)) {
        log.driverAuthFailed(prefix);
      } else {
        log.driverError(prefix, msg, durationStr);
      }
      return result;
    }

    const responseText = this.parseJsonResponse(result.output);
    if (responseText === null) {
      log.driverError(prefix, "failed to parse JSON response from opencode", durationStr);
      return { ...result, success: false };
    }

    for (const line of responseText.split("\n")) {
      log.driver(prefix, line);
    }
    log.driverSuccess(prefix, durationStr);

    return { ...result, output: responseText };
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    // OpenCode does not currently support session continuity
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }

  private parseJsonResponse(output: string): string | null {
    const trimmed = output.trim();
    if (!trimmed) return null;

    // The JSON line is the last non-empty line of stdout
    const lines = trimmed.split("\n");
    for (let i = lines.length - 1; i >= 0; i--) {
      const line = lines[i].trim();
      if (!line) continue;
      try {
        const parsed = JSON.parse(line);
        if (typeof parsed.response === "string") {
          return parsed.response;
        }
      } catch {
        // Not valid JSON, keep searching
      }
    }
    return null;
  }
}
