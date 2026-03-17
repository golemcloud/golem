import { spawn, ChildProcess } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";

export interface WatcherEvent {
  path: string;
  skillName: string;
  timestamp: number;
}

export class SkillWatcher {
  private children: ChildProcess[] = [];
  private activatedSkills: Set<string> = new Set();
  private activatedEvents: WatcherEvent[] = [];
  private skillsDir: string;
  private extraDirs: string[] = [];
  private atimeBaselines: Map<string, { skillName: string; atimeMs: number }> =
    new Map();

  constructor(skillsDir: string) {
    this.skillsDir = path.resolve(skillsDir);
  }

  addWatchDir(dir: string): void {
    this.extraDirs.push(path.resolve(dir));
  }

  async start(): Promise<void> {
    const dirs = [this.skillsDir, ...this.extraDirs];
    const platform = process.platform;

    for (const dir of dirs) {
      const child = this.spawnWatcher(platform, dir);
      if (!child) continue;
      this.children.push(child);

      child.stdout?.on("data", (data) => {
        const output = data.toString();
        const paths =
          platform === "darwin" ? output.split("\0") : output.split("\n");

        for (const p of paths) {
          if (!p) continue;
          const skillName = this.pathToSkillName(p.trim());
          if (skillName) {
            this.activatedSkills.add(skillName);
            this.activatedEvents.push({
              path: p.trim(),
              skillName,
              timestamp: Date.now(),
            });
          }
        }
      });

      child.on("error", (err) => {
        console.error(`SkillWatcher error: ${err.message}`);
      });
    }
  }

  private spawnWatcher(
    platform: NodeJS.Platform,
    dir: string,
  ): ChildProcess | null {
    if (platform === "linux") {
      return spawn("inotifywait", [
        "-m",
        "-r",
        "-e",
        "access",
        "--format",
        "%w%f",
        dir,
      ]);
    } else if (platform === "darwin") {
      return spawn("fswatch", ["-0", "-a", "-L", dir]);
    }
    console.warn(
      `SkillWatcher: Unsupported platform ${platform}, activation tracking disabled.`,
    );
    return null;
  }

  async stop(): Promise<void> {
    for (const child of this.children) {
      child.kill();
    }
    this.children = [];
  }

  getActivatedSkills(): string[] {
    return Array.from(this.activatedSkills);
  }

  markBaseline(): number {
    return this.activatedEvents.length;
  }

  /**
   * Snapshot atime of all SKILL.md files across watched directories.
   * Call this before the agent step runs. Returns a baseline token.
   */
  async snapshotAtimes(): Promise<void> {
    this.atimeBaselines.clear();
    const dirs = [this.skillsDir, ...this.extraDirs];
    for (const dir of dirs) {
      await this.collectSkillAtimes(dir);
    }
  }

  /**
   * Compare current atimes against the snapshot.
   * Returns skill names whose SKILL.md files had atime updated since the snapshot.
   */
  async getSkillsWithChangedAtime(): Promise<
    { skillName: string; path: string }[]
  > {
    const changed: { skillName: string; path: string }[] = [];
    for (const [filePath, baseline] of this.atimeBaselines) {
      try {
        const stat = await fs.stat(filePath);
        if (stat.atimeMs > baseline.atimeMs) {
          changed.push({ skillName: baseline.skillName, path: filePath });
        }
      } catch {
        // File may have been removed
      }
    }
    return changed;
  }

  getActivatedEventsSince(baseline: number): WatcherEvent[] {
    return this.activatedEvents.slice(baseline);
  }

  clearActivatedSkills(): void {
    this.activatedSkills.clear();
    this.activatedEvents = [];
  }

  pathToSkillName(filePath: string): string | null {
    if (!filePath.endsWith("SKILL.md")) return null;
    // Extract the parent directory name
    const dirName = path.dirname(filePath);
    return path.basename(dirName);
  }

  private async collectSkillAtimes(dir: string): Promise<void> {
    try {
      const entries = await fs.readdir(dir, { withFileTypes: true });
      for (const entry of entries) {
        if (!entry.isDirectory()) continue;
        const skillFile = path.join(dir, entry.name, "SKILL.md");
        try {
          const stat = await fs.stat(skillFile);
          // On macOS APFS, atime only updates when atime <= mtime (relatime).
          // Reset atime to before mtime so the next read triggers an update.
          const oldAtime = new Date(stat.mtimeMs - 1000);
          await fs.utimes(skillFile, oldAtime, stat.mtime);
          this.atimeBaselines.set(skillFile, {
            skillName: entry.name,
            atimeMs: stat.mtimeMs - 1000,
          });
        } catch {
          // No SKILL.md in this directory
        }
      }
    } catch {
      // Directory doesn't exist yet
    }
  }
}
