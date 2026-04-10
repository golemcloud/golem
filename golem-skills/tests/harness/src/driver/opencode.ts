import { BaseAgentDriver, AgentResult } from "./base.js";

export class OpenCodeAgentDriver extends BaseAgentDriver {
  protected readonly skillDirs = [".claude/skills", ".agents/skills"];

  private buildArgs(prompt: string): string[] {
    const args = ["run"];
    const model = process.env.OPENCODE_MODEL;
    if (model) {
      args.push("-m", model);
    }
    args.push(prompt);
    return args;
  }

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    return this.runCommand("opencode", this.buildArgs(prompt), timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    // OpenCode does not currently support session continuity
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }
}
