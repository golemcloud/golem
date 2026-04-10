import { BaseAgentDriver, AgentResult } from "./base.js";
import * as log from "../log.js";

export class OpenCodeAgentDriver extends BaseAgentDriver {
  protected readonly driverName = "opencode";
  protected readonly skillDirs = [".agents/skills"];
  private lastSessionId: string | null = null;

  private buildArgs(prompt: string, isFollowup: boolean): string[] {
    const args = [
      "run",
      "--format",
      "json",
      "--dangerously-skip-permissions",
    ];
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

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    this.lastSessionId = null;
    return this.executeOpencode(prompt, false, timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    return this.executeOpencode(prompt, true, timeout);
  }

  async teardown(): Promise<void> {
    this.lastSessionId = null;
  }

  private async executeOpencode(
    prompt: string,
    isFollowup: boolean,
    timeout: number,
  ): Promise<AgentResult> {
    const prefix = this.logPrefix;
    const result = await this.runCommand(
      "opencode",
      this.buildArgs(prompt, isFollowup),
      timeout,
    );
    const durationStr = `(${result.durationSeconds.toFixed(1)}s)`;

    if (!result.success) {
      const msg = result.output || "Unknown error";
      if (/command not found|ENOENT/i.test(msg)) {
        log.driverFatal(prefix, "opencode CLI not installed");
      } else if (/\bauthentication\s+failed\b|invalid.*api.key|unauthorized/i.test(msg)) {
        log.driverAuthFailed(prefix);
      } else {
        log.driverError(prefix, msg, durationStr);
      }
      return result;
    }

    // Parse JSON events from the output to extract session ID and text
    const responseText = this.parseJsonEvents(result.output);
    if (responseText !== null) {
      log.driverSuccess(prefix, durationStr);
      return { ...result, output: responseText };
    }

    // If no JSON events were found, return the raw output
    log.driverSuccess(prefix, durationStr);
    return result;
  }

  private parseJsonEvents(output: string): string | null {
    const trimmed = output.trim();
    if (!trimmed) return null;

    const textParts: string[] = [];
    let foundJson = false;

    for (const line of trimmed.split("\n")) {
      const l = line.trim();
      if (!l) continue;
      try {
        const event = JSON.parse(l) as Record<string, unknown>;
        foundJson = true;

        // Extract session ID from session events
        if (event.type === "session.created" || event.type === "session.resumed") {
          const sessionId =
            (event.session_id as string) ??
            ((event.session as Record<string, unknown>)?.id as string);
          if (sessionId) {
            this.lastSessionId = sessionId;
          }
        }

        // Extract assistant text content
        if (event.type === "message.created" || event.type === "message.updated") {
          const message = event.message as Record<string, unknown> | undefined;
          if (message?.role === "assistant") {
            const content = message.content;
            if (typeof content === "string") {
              textParts.push(content);
            } else if (Array.isArray(content)) {
              for (const block of content as Record<string, unknown>[]) {
                if (block.type === "text" && typeof block.text === "string") {
                  textParts.push(block.text);
                }
              }
            }
          }
        }

        // Also check for the "response" field used by some opencode versions
        if (typeof event.response === "string") {
          textParts.push(event.response);
        }
      } catch {
        // Not JSON — raw output line, already logged by the base driver
      }
    }

    if (!foundJson) return null;
    return textParts.join("\n") || "";
  }
}
