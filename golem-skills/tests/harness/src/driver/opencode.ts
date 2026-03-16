import { BaseAgentDriver, AgentResult } from "./base.js";

export class OpenCodeAgentDriver extends BaseAgentDriver {
  protected readonly skillDirs = [".claude/skills", ".agents/skills"];

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    return this.runCommand("opencode", ["run", prompt], timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    // OpenCode does not currently support session continuity
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }
}
