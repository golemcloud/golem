import { BaseAgentDriver, AgentResult } from './base.js';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';

export class OpenCodeAgentDriver extends BaseAgentDriver {
  async setup(workspace: string, skillsDir: string): Promise<void> {
    await super.setup(workspace, skillsDir);

    // Symlink skills into .claude/skills/ (same as Claude driver)
    const claudeSkillsDir = path.join(workspace, '.claude', 'skills');
    await fs.mkdir(claudeSkillsDir, { recursive: true });

    // Also symlink into .agents/skills/
    const agentsSkillsDir = path.join(workspace, '.agents', 'skills');
    await fs.mkdir(agentsSkillsDir, { recursive: true });

    const skills = await fs.readdir(skillsDir, { withFileTypes: true });
    for (const dirent of skills) {
      if (dirent.isDirectory()) {
        const skillName = dirent.name;
        const sourceDir = path.resolve(skillsDir, skillName);
        const sourceFile = path.join(sourceDir, 'SKILL.md');

        try {
          await fs.access(sourceFile);
        } catch {
          continue;
        }

        // Link to .claude/skills/
        const claudeDestDir = path.join(claudeSkillsDir, skillName);
        await fs.mkdir(claudeDestDir, { recursive: true });
        await fs.symlink(sourceFile, path.join(claudeDestDir, 'SKILL.md')).catch(() => {});

        // Link to .agents/skills/
        const agentsDestDir = path.join(agentsSkillsDir, skillName);
        await fs.mkdir(agentsDestDir, { recursive: true });
        await fs.symlink(sourceFile, path.join(agentsDestDir, 'SKILL.md')).catch(() => {});
      }
    }
  }

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    return this.runCommand('opencode', ['run', prompt], timeout);
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    // OpenCode session continuity TBD
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }
}
