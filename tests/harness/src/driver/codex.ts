import { BaseAgentDriver, AgentResult } from './base.js';

export class CodexAgentDriver extends BaseAgentDriver {
  protected readonly skillDirs = ['.agents/skills'];
  private lastSessionId: string | null = null;

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    const result = await this.runCommand(
      'codex',
      [
        'exec',
        '--dangerously-bypass-approvals-and-sandbox',
        '--json',
        prompt,
      ],
      timeout
    );

    this.lastSessionId = this.extractSessionId(result.output);

    if (!result.success && result.output.includes('command not found')) {
      result.output = `Codex CLI not installed. ${result.output}`;
    }
    if (!result.success && /auth|api key|unauthorized/i.test(result.output)) {
      result.output = `Codex authentication failed. ${result.output}`;
    }
    return result;
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    if (!this.lastSessionId) {
      return this.sendPrompt(prompt, timeout);
    }
    return this.runCommand(
      'codex',
      [
        'exec',
        'resume',
        this.lastSessionId,
        '--dangerously-bypass-approvals-and-sandbox',
        '--json',
        prompt,
      ],
      timeout
    );
  }

  async teardown(): Promise<void> {
    this.lastSessionId = null;
  }

  private extractSessionId(output: string): string | null {
    const lines = output.trim().split('\n');
    for (let i = lines.length - 1; i >= 0; i--) {
      try {
        const parsed = JSON.parse(lines[i]) as Record<string, unknown>;
        if (typeof parsed.session_id === 'string' && parsed.session_id.length > 0) {
          return parsed.session_id;
        }
        if (typeof parsed.conversation_id === 'string' && parsed.conversation_id.length > 0) {
          return parsed.conversation_id;
        }
      } catch {
        // Not JSON, skip
      }
    }
    return null;
  }
}
