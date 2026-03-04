import { BaseAgentDriver, AgentResult } from './base.js';

export class GeminiAgentDriver extends BaseAgentDriver {
  async setup(workspace: string, skillsDir: string): Promise<void> {
    await super.setup(workspace, skillsDir);
    // Gemini skill reading mechanism TBD
  }

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    return this.runCommand('gemini', ['-p', prompt], timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    // Gemini might not have persistent sessions in CLI mode easily
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }
}
