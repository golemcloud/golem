import * as fs from "node:fs/promises";
import * as path from "node:path";

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
