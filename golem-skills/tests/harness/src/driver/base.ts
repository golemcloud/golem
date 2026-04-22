import { spawn, type ChildProcess } from "node:child_process";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as log from "../log.js";

/**
 * Kill a child process and its entire process tree.
 * Uses negative PID to send SIGTERM to the process group, falling back to
 * killing just the process. Schedules a SIGKILL if the tree doesn't exit
 * within 5 seconds.
 */
export function killProcessTree(child: ChildProcess): void {
  try {
    if (child.pid) {
      process.kill(-child.pid, "SIGTERM");
    } else {
      child.kill("SIGTERM");
    }
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {
      // Already dead
    }
  }

  setTimeout(() => {
    try {
      if (child.pid) {
        process.kill(-child.pid, "SIGKILL");
      } else {
        child.kill("SIGKILL");
      }
    } catch {
      // Already dead
    }
  }, 5000);
}

export interface AgentResult {
  success: boolean;
  output: string;
  durationSeconds: number;
  exitCode: number | null;
  timedOut?: boolean;
  timeoutKind?: "step" | "idle";
}

export interface DriverTimeoutOptions {
  stepTimeoutSeconds: number;
  idleTimeoutSeconds?: number; // default: 300 (5 min)
  heartbeatIntervalSeconds?: number; // default: 30
}

export interface AgentDriver {
  setup(workspace: string, bootstrapSkillSourceDirs: string[]): Promise<void>;
  sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult>;
  sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult>;
  teardown(): Promise<void>;
  /** Update the working directory used for subsequent agent invocations. */
  setWorkingDirectory(dir: string): void;

  /**
   * Return all skills activated during the current prompt session.
   * Drivers that can detect skill activations natively (e.g. via tool-use
   * events) should override this. When `undefined` is returned the executor
   * falls back to the filesystem-based `SkillWatcher`.
   *
   * Skills accumulate across followup prompts in the same agent thread. The
   * executor resets tracking when a prompt starts a fresh session (the first
   * prompt in a scenario, or any prompt with `continueSession: false`).
   */
  getActivatedSkills(): string[] | undefined;

  /**
   * Clear the driver's internal list of activated skills.
   * Called both during teardown and when the executor starts a fresh prompt
   * session.
   */
  resetActivatedSkills(): void;
}

export const DEFAULT_IDLE_TIMEOUT_SECONDS = 300;
export const DEFAULT_HEARTBEAT_INTERVAL_SECONDS = 30;

export class ActivityMonitor {
  private lastActivityTime: number;
  private readonly startTime: number;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private stepTimer: ReturnType<typeof setTimeout> | null = null;
  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private settled = false;
  private _timeoutKind: "step" | "idle" | undefined = undefined;
  private readonly prefix: string;
  private readonly idleTimeoutMs: number;
  private readonly idleTimeoutSeconds: number;
  private readonly onTimeout: (kind: "step" | "idle") => void;

  constructor(
    prefix: string,
    opts: DriverTimeoutOptions,
    onTimeout: (kind: "step" | "idle") => void,
  ) {
    this.prefix = prefix;
    this.startTime = Date.now();
    this.lastActivityTime = this.startTime;
    this.onTimeout = onTimeout;

    this.idleTimeoutSeconds = opts.idleTimeoutSeconds ?? DEFAULT_IDLE_TIMEOUT_SECONDS;
    const heartbeatSeconds = opts.heartbeatIntervalSeconds ?? DEFAULT_HEARTBEAT_INTERVAL_SECONDS;
    this.idleTimeoutMs = this.idleTimeoutSeconds * 1000;

    // Heartbeat timer — only logs when idle (no output since last heartbeat)
    this.heartbeatTimer = setInterval(() => {
      if (this.settled) return;
      const idleMs = Date.now() - this.lastActivityTime;
      if (idleMs >= heartbeatSeconds * 1000) {
        const elapsed = Math.round((Date.now() - this.startTime) / 1000);
        log.driverHeartbeat(this.prefix, elapsed);
      }
    }, heartbeatSeconds * 1000);

    // Step timeout
    this.stepTimer = setTimeout(() => {
      this.triggerTimeout("step", opts.stepTimeoutSeconds);
    }, opts.stepTimeoutSeconds * 1000);

    // Idle timeout (resettable via noteActivity)
    this.armIdleTimer();
  }

  private triggerTimeout(kind: "step" | "idle", displaySeconds?: number): void {
    if (this.settled) return;
    this._timeoutKind = kind;
    this.settled = true;
    if (kind === "step") {
      log.driverTimeout(this.prefix, displaySeconds ?? 0);
    } else {
      log.driverIdleTimeout(this.prefix, displaySeconds ?? this.idleTimeoutSeconds);
    }
    this.clearTimers();
    this.onTimeout(kind);
  }

  private armIdleTimer(): void {
    if (this.idleTimeoutMs <= 0 || this.settled) return;
    if (this.idleTimer) clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => {
      this.triggerTimeout("idle");
    }, this.idleTimeoutMs);
  }

  /** Call this whenever stdout/stderr data is received from the agent. */
  noteActivity(): void {
    if (this.settled) return;
    this.lastActivityTime = Date.now();
    this.armIdleTimer();
  }

  /** Call this when the process exits or the stream ends. Clears all timers. */
  finish(): void {
    if (this.settled) return;
    this.settled = true;
    this.clearTimers();
  }

  get isTimedOut(): boolean {
    return this._timeoutKind !== undefined;
  }

  get timeoutKind(): "step" | "idle" | undefined {
    return this._timeoutKind;
  }

  /** Build a human-readable timeout message matching the actual timeout kind. */
  formatTimeoutMessage(fallbackPrefix: string): string {
    if (this._timeoutKind === "idle") {
      return `${fallbackPrefix} idle timeout — no output for ${this.idleTimeoutSeconds}s`;
    }
    const elapsed = Math.round((Date.now() - this.startTime) / 1000);
    return `${fallbackPrefix} timed out after ${elapsed}s`;
  }

  private clearTimers(): void {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
    if (this.stepTimer) {
      clearTimeout(this.stepTimer);
      this.stepTimer = null;
    }
    if (this.idleTimer) {
      clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
  }
}

export abstract class BaseAgentDriver implements AgentDriver {
  protected workspace: string = ".";
  protected bootstrapSkillSourceDirs: string[] = [];

  /** Short tag used in log output (e.g. "claude-code", "amp") */
  protected abstract readonly driverName: string;

  /** Directories relative to workspace where the bootstrap skill should be copied. */
  protected abstract readonly skillDirs: string[];

  /** Returns the driver log tag, e.g. `claude-code` */
  protected get logPrefix(): string {
    return this.driverName;
  }

  async setup(workspace: string, bootstrapSkillSourceDirs: string[]): Promise<void> {
    this.workspace = workspace;
    this.bootstrapSkillSourceDirs = bootstrapSkillSourceDirs;
    await this.seedBootstrapSkills();
  }

  setWorkingDirectory(dir: string): void {
    this.workspace = dir;
  }

  protected async seedBootstrapSkills(): Promise<void> {
    for (const bootstrapSkillSourceDir of this.bootstrapSkillSourceDirs) {
      const sourceDir = path.resolve(bootstrapSkillSourceDir);
      await fs.access(path.join(sourceDir, "SKILL.md"));
      const skillName = path.basename(sourceDir);

      for (const targetDir of this.skillDirs) {
        const destDir = path.join(this.workspace, targetDir, skillName);
        await fs.mkdir(path.dirname(destDir), { recursive: true });
        await fs.cp(sourceDir, destDir, { recursive: true });
      }
    }
  }

  getActivatedSkills(): string[] | undefined {
    return undefined;
  }

  resetActivatedSkills(): void {
    // No-op by default; drivers that track skills natively override this.
  }

  abstract sendPrompt(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult>;
  abstract sendFollowup(prompt: string, opts: DriverTimeoutOptions): Promise<AgentResult>;
  abstract teardown(): Promise<void>;

  protected async runCommand(
    command: string,
    args: string[],
    opts: DriverTimeoutOptions,
    cwd?: string,
  ): Promise<AgentResult> {
    const startTime = Date.now();

    return new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd: cwd || this.workspace,
        detached: true,
        env: { ...process.env },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let output = "";
      let stdoutBuf = "";
      let stderrBuf = "";
      const prefix = this.logPrefix;

      const monitor = new ActivityMonitor(prefix, opts, (_kind) => {
        killProcessTree(child);
      });

      child.stdout?.on("data", (data) => {
        monitor.noteActivity();
        const chunk = data.toString();
        output += chunk;
        stdoutBuf += chunk;
        const lines = stdoutBuf.split("\n");
        stdoutBuf = lines.pop()!;
        for (const line of lines) {
          log.driver(prefix, line);
        }
      });
      child.stderr?.on("data", (data) => {
        monitor.noteActivity();
        const chunk = data.toString();
        output += chunk;
        stderrBuf += chunk;
        const lines = stderrBuf.split("\n");
        stderrBuf = lines.pop()!;
        for (const line of lines) {
          log.driverErr(prefix, line);
        }
      });

      child.on("close", (exitCode) => {
        monitor.finish();
        if (stdoutBuf) log.driver(prefix, stdoutBuf);
        if (stderrBuf) log.driverErr(prefix, stderrBuf);
        const durationSeconds = (Date.now() - startTime) / 1000;
        const timedOut = monitor.isTimedOut;
        resolve({
          success: !timedOut && exitCode === 0,
          output: timedOut ? `${monitor.formatTimeoutMessage("Agent")}. ${output}` : output,
          durationSeconds,
          exitCode: timedOut ? null : exitCode,
          timedOut,
          timeoutKind: monitor.timeoutKind,
        });
      });

      child.on("error", (err) => {
        monitor.finish();
        const durationSeconds = (Date.now() - startTime) / 1000;
        resolve({
          success: false,
          output: output + (err.message || "Unknown error"),
          durationSeconds,
          exitCode: null,
        });
      });
    });
  }
}
