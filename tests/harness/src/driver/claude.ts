import { BaseAgentDriver, AgentResult } from './base.js';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';

export class ClaudeAgentDriver extends BaseAgentDriver {
  private sessionId: string | null = null;

  async setup(workspace: string, skillsDir: string): Promise<void> {
    await super.setup(workspace, skillsDir);
    // Link skills to .claude/skills in the workspace
    const claudeSkillsDir = path.join(workspace, '.claude', 'skills');
    await fs.mkdir(claudeSkillsDir, { recursive: true });

    const skills = await fs.readdir(skillsDir, { withFileTypes: true });
    for (const dirent of skills) {
      if (dirent.isDirectory()) {
        const skillName = dirent.name;
        const sourceDir = path.resolve(skillsDir, skillName);
        const destDir = path.join(claudeSkillsDir, skillName);
        
        await fs.mkdir(destDir, { recursive: true });
        
        const sourceFile = path.join(sourceDir, 'SKILL.md');
        const destFile = path.join(destDir, 'SKILL.md');
        
        try {
          await fs.access(sourceFile);
          await fs.symlink(sourceFile, destFile).catch(() => {});
        } catch {
          // Ignore if SKILL.md doesn't exist
        }
      }
    }
  }

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    const result = await this.runCommand(
      'claude',
      ['--print', '--output-format', 'json', '--permission-mode', 'bypassPermissions', '--max-turns', '25', prompt],
      timeout
    );
    // Try to parse sessionId from output if Claude provides one
    const parsed = this.tryParseJson(result.output);
    if (parsed && typeof parsed === 'object' && 'sessionId' in parsed) {
      const sessionId = parsed.sessionId;
      this.sessionId = typeof sessionId === 'string' && sessionId.length > 0 ? sessionId : null;
    }
    if (!result.success && result.output.includes('command not found')) {
      result.output = `Claude CLI not installed. ${result.output}`;
    }
    if (!result.success && /auth|api key|unauthorized/i.test(result.output)) {
      result.output = `Claude authentication failed. ${result.output}`;
    }
    return result;
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    if (!this.sessionId) {
      return this.sendPrompt(prompt, timeout);
    }
    return this.runCommand(
      'claude',
      ['--print', '--permission-mode', 'bypassPermissions', '--resume', '--session-id', this.sessionId, prompt],
      timeout
    );
  }

  async teardown(): Promise<void> {
    // Cleanup if necessary
  }

  private tryParseJson(output: string): Record<string, unknown> | null {
    try {
      return JSON.parse(output);
    } catch {
      const lines = output.trim().split('\n');
      for (let i = lines.length - 1; i >= 0; i--) {
        try {
          return JSON.parse(lines[i]) as Record<string, unknown>;
        } catch {
          // Continue
        }
      }
      return null;
    }
  }
}
