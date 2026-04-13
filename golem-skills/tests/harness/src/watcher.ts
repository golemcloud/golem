import { spawn, ChildProcess } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as log from "./log.js";

export interface WatcherEvent {
  path: string;
  skillName: string;
  timestamp: number;
}

export class SkillWatcher {
  private children: ChildProcess[] = [];
  private activatedSkills: Set<string> = new Set();
  private activatedEvents: WatcherEvent[] = [];
  private rootDir: string;
  private extraDirs: string[] = [];
  private atimeBaselines: Map<string, { skillName: string; atimeMs: number }> = new Map();
  private warnedMissingTool = false;
  private suppressEvents = false;

  constructor(rootDir: string) {
    this.rootDir = path.resolve(rootDir);
  }

  addWatchDir(dir: string): void {
    this.extraDirs.push(path.resolve(dir));
  }

  async start(): Promise<void> {
    const dirs = [this.rootDir, ...this.extraDirs];
    const platform = process.platform;

    for (const dir of dirs) {
      const child = this.spawnWatcher(platform, dir);
      if (!child) continue;
      this.children.push(child);
      let stdoutBuffer = "";

      child.stdout?.on("data", (data) => {
        stdoutBuffer += data.toString();
        const delimiter = platform === "darwin" ? "\0" : "\n";
        const paths = stdoutBuffer.split(delimiter);
        stdoutBuffer = paths.pop() ?? "";

        for (const p of paths) {
          if (!p) continue;
          const skillName = this.pathToSkillName(p.trim());
          if (skillName && !this.suppressEvents) {
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
        if ((err as NodeJS.ErrnoException).code === "ENOENT") {
          if (!this.warnedMissingTool) {
            this.warnedMissingTool = true;
            const tool = platform === "darwin" ? "fswatch" : "inotifywait";
            log.warn(
              `SkillWatcher: "${tool}" not found; falling back to atime-based detection. ` +
                `Install ${tool} for real-time skill activation tracking.`,
            );
          }
        } else {
          log.error(`SkillWatcher error: ${err.message}`);
        }
      });
    }
  }

  private spawnWatcher(platform: NodeJS.Platform, dir: string): ChildProcess | null {
    if (platform === "linux") {
      return spawn("inotifywait", ["-m", "-r", "-e", "access", "--format", "%w%f", dir]);
    } else if (platform === "darwin") {
      return null;
    }
    log.warn(`SkillWatcher: Unsupported platform ${platform}, activation tracking disabled.`);
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
    this.suppressEvents = true;
    try {
      this.atimeBaselines.clear();
      const dirs = [this.rootDir, ...this.extraDirs];
      for (const dir of dirs) {
        await this.collectSkillAtimes(dir);
      }
    } finally {
      this.suppressEvents = false;
    }
  }

  /**
   * Compare current atimes against the snapshot.
   * Returns skill names whose SKILL.md files had atime updated since the snapshot.
   */
  async getSkillsWithChangedAtime(): Promise<{ skillName: string; path: string }[]> {
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
    const normalizedPath = filePath.replace(/\\/g, "/");
    if (!normalizedPath.endsWith("/SKILL.md") && normalizedPath !== "SKILL.md") {
      return null;
    }

    if (!normalizedPath.includes("/.agents/skills/")) {
      return null;
    }

    const segments = normalizedPath.split("/").filter((segment) => segment.length > 0);
    if (segments.length < 2) {
      return null;
    }

    return segments[segments.length - 2];
  }

  private async collectSkillAtimes(dir: string): Promise<void> {
    try {
      const entries = await fs.readdir(dir, { withFileTypes: true });
      for (const entry of entries) {
        const entryPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          await this.collectSkillAtimes(entryPath);
          continue;
        }

        if (entry.name !== "SKILL.md") {
          continue;
        }

        const skillName = this.pathToSkillName(entryPath);
        if (!skillName) {
          continue;
        }

        const stat = await fs.stat(entryPath);
        // On macOS APFS, atime only updates when atime <= mtime (relatime).
        // Reset atime to before mtime so the next read triggers an update.
        const oldAtime = new Date(stat.mtimeMs - 1000);
        await fs.utimes(entryPath, oldAtime, stat.mtime);
        this.atimeBaselines.set(entryPath, {
          skillName,
          atimeMs: stat.mtimeMs - 1000,
        });
      }
    } catch {
      // Directory doesn't exist yet
    }
  }
}
