import * as fs from "node:fs/promises";
import * as path from "node:path";

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
