import { BaseAgentDriver, AgentResult } from "./base.js";

export class GeminiAgentDriver extends BaseAgentDriver {
  protected readonly skillDirs = [".gemini/skills"];
  protected readonly skillLinkMode = "copy" as const;

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    return this.runCommand(
      "gemini",
      ["--approval-mode", "yolo", "--output-format", "json", "-p", prompt],
      timeout,
    );
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }
}
