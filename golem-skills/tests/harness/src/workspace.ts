import * as fs from "node:fs/promises";
import * as path from "node:path";
import { accessSync, constants } from "node:fs";
import { spawn, type ChildProcess } from "node:child_process";

/**
 * Detect if the current working directory is within a checked-out golem repository.
 * Walks up from `startDir` looking for a directory containing both
 * `sdks/rust/golem-rust` and `sdks/ts/packages` (the same markers used by golem-cli).
 * Returns the workspace root path if found, or undefined otherwise.
 */
export async function detectGolemWorkspaceRoot(
  startDir?: string,
): Promise<string | undefined> {
  let dir = path.resolve(startDir ?? process.cwd());
  const { root } = path.parse(dir);

  while (true) {
    try {
      const [rustSdk, tsSdk] = await Promise.all([
        fs.stat(path.join(dir, "sdks", "rust", "golem-rust")),
        fs.stat(path.join(dir, "sdks", "ts", "packages")),
      ]);
      if (rustSdk.isDirectory() && tsSdk.isDirectory()) {
        return dir;
      }
    } catch {
      // Markers not found at this level, keep walking up
    }

    if (dir === root) break;
    dir = path.dirname(dir);
  }

  return undefined;
}

/**
 * Find the directory containing golem.yaml within a workspace.
 * Checks the workspace root first, then immediate subdirectories.
 * Returns the workspace root as fallback if no golem.yaml is found.
 */
export async function findGolemAppDir(workspace: string): Promise<string> {
  // Check workspace root first
  try {
    await fs.access(path.join(workspace, "golem.yaml"));
    return workspace;
  } catch {
    // Not in root, search immediate subdirectories
  }

  const entries = await fs
    .readdir(workspace, { withFileTypes: true })
    .catch(() => []);
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    const candidate = path.join(workspace, entry.name);
    try {
      await fs.access(path.join(candidate, "golem.yaml"));
      return candidate;
    } catch {
      // Continue searching
    }
  }

  // Fall back to workspace root
  return workspace;
}

/**
 * Given a GOLEM_PATH, find the target directory containing the `golem` binary.
 * Prefers `target/release` if a `golem` executable exists there, otherwise
 * falls back to `target/debug`. Throws if neither contains the binary.
 */
export function resolveGolemTargetDir(golemPath: string): string {
  const releaseDir = path.join(golemPath, "target", "release");
  const debugDir = path.join(golemPath, "target", "debug");

  try {
    accessSync(path.join(releaseDir, "golem"), constants.X_OK);
    return releaseDir;
  } catch {
    // No release build
  }

  try {
    accessSync(path.join(debugDir, "golem"), constants.X_OK);
    return debugDir;
  } catch {
    // No debug build either
  }

  throw new Error(
    `No golem binary found in GOLEM_PATH (${golemPath}).\n` +
    `Checked:\n` +
    `  - ${path.join(releaseDir, "golem")}\n` +
    `  - ${path.join(debugDir, "golem")}\n` +
    `Build golem first with: cargo build -p golem`,
  );
}

/**
 * Manages a local Golem server process. The harness always starts its own
 * server with `--data-dir` and `--clean` for isolation. If a server is
 * already running on the target port, startup fails to avoid surprises.
 */
export class GolemServer {
  private serverProcess: ChildProcess | null = null;
  private lastPort: number | null = null;
  private lastDataDir: string | null = null;

  /**
   * Check whether a server is already listening on the given port.
   */
  async isRunning(port: number): Promise<boolean> {
    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 5000);
      const res = await fetch(`http://localhost:${port}/healthcheck`, {
        signal: controller.signal,
      });
      clearTimeout(timeout);
      return res.ok;
    } catch {
      return false;
    }
  }

  /**
   * Start a Golem server. Fails if one is already running on the port.
   * Spawns `golem server run --data-dir <dataDir> --clean` and polls
   * the healthcheck endpoint until it responds (up to ~60 s).
   */
  async start(port: number, dataDir: string): Promise<void> {
    if (await this.isRunning(port)) {
      throw new Error(
        `A Golem server is already running on port ${port}.\n` +
        `The skill test harness needs to manage its own server instance.\n` +
        `Please stop the existing server and try again.`,
      );
    }

    this.lastPort = port;
    this.lastDataDir = dataDir;

    await fs.mkdir(dataDir, { recursive: true });

    this.serverProcess = spawn(
      "golem",
      ["server", "run", "--data-dir", dataDir, "--clean"],
      {
        stdio: ["ignore", "pipe", "pipe"],
        detached: true,
      },
    );

    this.serverProcess.stdout?.on("data", (data: Buffer) => {
      process.stdout.write(`[golem-server] ${data.toString()}`);
    });
    this.serverProcess.stderr?.on("data", (data: Buffer) => {
      process.stderr.write(`[golem-server] ${data.toString()}`);
    });

    this.serverProcess.on("error", (err) => {
      console.error(`Golem server process error: ${err.message}`);
      this.serverProcess = null;
    });

    this.serverProcess.on("exit", (code, signal) => {
      if (code !== null && code !== 0) {
        console.error(`Golem server exited with code ${code}`);
      } else if (signal) {
        // Expected when we stop it ourselves
      }
      this.serverProcess = null;
    });

    // Poll healthcheck
    const maxAttempts = 30;
    const delayMs = 2000;
    for (let i = 1; i <= maxAttempts; i++) {
      await new Promise((resolve) => setTimeout(resolve, delayMs));
      if (this.serverProcess === null) {
        throw new Error(
          "Golem server process exited before becoming ready. Check the output above for errors.",
        );
      }
      if (await this.isRunning(port)) {
        return;
      }
    }

    // Timed out — kill and fail
    await this.stop();
    throw new Error(
      `Golem server did not become ready on port ${port} within ${(maxAttempts * delayMs) / 1000}s.`,
    );
  }

  /**
   * Stop the server if we started it. No-op if not running.
   * Returns a promise that resolves once the process has exited.
   */
  async stop(): Promise<void> {
    if (!this.serverProcess) return;

    const proc = this.serverProcess;
    this.serverProcess = null;

    // Register the exit listener before sending signals to avoid race conditions
    const exited = new Promise<void>((resolve) => {
      if (proc.exitCode !== null || proc.signalCode !== null) {
        resolve();
      } else {
        proc.on("exit", () => resolve());
      }
    });

    // Kill the process tree if possible (negative PID sends to process group),
    // otherwise fall back to killing just the process.
    try {
      if (proc.pid) {
        process.kill(-proc.pid, "SIGTERM");
      } else {
        proc.kill("SIGTERM");
      }
    } catch {
      proc.kill("SIGTERM");
    }

    // Force kill after 5 seconds if still alive
    const forceKill = setTimeout(() => {
      try {
        if (proc.pid) {
          process.kill(-proc.pid, "SIGKILL");
        } else {
          proc.kill("SIGKILL");
        }
      } catch {
        // Already dead
      }
    }, 5000);

    await exited;
    clearTimeout(forceKill);
  }

  /**
   * Stop the server and start it again with the same parameters.
   */
  async restart(): Promise<void> {
    if (this.lastPort === null || this.lastDataDir === null) {
      throw new Error("Cannot restart: server was never started.");
    }
    await this.stop();
    await this.start(this.lastPort, this.lastDataDir);
  }
}
