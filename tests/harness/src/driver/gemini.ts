import { BaseAgentDriver, AgentResult } from "./base.js";
import * as path from "node:path";
import * as fs from "node:fs/promises";

export class GeminiAgentDriver extends BaseAgentDriver {
  async setup(workspace: string, skillsDir: string): Promise<void> {
    await super.setup(workspace, skillsDir);

    const geminiSkillsDir = path.join(workspace, ".gemini", "skills");
    await fs.mkdir(geminiSkillsDir, { recursive: true });

    const skills = await fs.readdir(skillsDir, { withFileTypes: true });
    for (const dirent of skills) {
      if (dirent.isDirectory()) {
        const skillName = dirent.name;
        const sourceDir = path.resolve(skillsDir, skillName);
        const destDir = path.join(geminiSkillsDir, skillName);

        await fs.mkdir(destDir, { recursive: true });

        const sourceFile = path.join(sourceDir, "SKILL.md");
        const destFile = path.join(destDir, "SKILL.md");

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
