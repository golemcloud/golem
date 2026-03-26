import * as fs from "node:fs/promises";
import * as path from "node:path";
import { BaseAgentDriver, AgentResult } from "./base.js";

export class GeminiAgentDriver extends BaseAgentDriver {
  protected readonly skillDirs = [".gemini/skills"];
  protected readonly skillLinkMode = "copy" as const;

  async sendPrompt(prompt: string, timeout: number): Promise<AgentResult> {
    const includeDirectories = await this.getIncludeDirectories();
    const includeArgs =
      includeDirectories.length > 0
        ? ["--include-directories", includeDirectories.join(",")]
        : [];

    return this.runCommand(
      "gemini",
      [
        "--approval-mode",
        "yolo",
        "--output-format",
        "json",
        ...includeArgs,
        "-p",
        prompt,
      ],
      timeout,
    );
  }

  async sendFollowup(prompt: string, timeout: number): Promise<AgentResult> {
    return this.sendPrompt(prompt, timeout);
  }

  async teardown(): Promise<void> {
    // Cleanup
  }

  private async getIncludeDirectories(): Promise<string[]> {
    const candidates = new Set<string>();

    // Gemini's rootDirectory is the current workspace. Add nearby roots explicitly
    // because the CLI's file tools can reject parent/auxiliary paths unless they
    // are included at launch time.
    candidates.add(path.dirname(this.workspace));
    candidates.add(path.join(this.workspace, ".gemini", "skills"));

    // Include immediate child directories when they already exist so followup
    // prompts can access scaffolded app subdirectories directly.
    const entries = await fs
      .readdir(this.workspace, { withFileTypes: true })
      .catch(() => []);
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      candidates.add(path.join(this.workspace, entry.name));
    }

    const includeDirectories: string[] = [];
    for (const candidate of candidates) {
      if (includeDirectories.length >= 5) {
        break;
      }
      try {
        const stat = await fs.stat(candidate);
        if (stat.isDirectory()) {
          includeDirectories.push(candidate);
        }
      } catch {
        // Skip paths that do not exist yet.
      }
    }

    return includeDirectories;
  }
}
