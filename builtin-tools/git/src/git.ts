import fs from "node:fs";
import { lstat, readFile, readlink, stat } from "node:fs/promises";
import path from "node:path";
import * as git from "isomorphic-git";
import type { TagObject } from "isomorphic-git";
import { GitIndexManager } from "isomorphic-git/managers";
import { FileSystem } from "isomorphic-git/models";
import { formatPatch, structuredPatch } from "diff";

export const MAX_DIFF_BYTES = 2 * 1024 * 1024;
export const MAX_DIFF_INPUT_BYTES = 8 * 1024 * 1024;
export const MAX_DIFF_EDIT_LENGTH = 100_000;
export const MAX_DIFF_LINES = 20_000;
const gitFs = new FileSystem(fs);

export type Repository = { cwd: string; dir: string; gitdir: string };
export type StatusEntry = {
  path: string;
  index: string;
  worktree: string;
  code: string;
};
export type LogEntry = {
  oid: string;
  message: string;
  authorName: string;
  authorEmail: string;
  timestamp: number;
};
export type BranchEntry = { name: string; current: boolean; oid: string };
export type MutationResult = { summary: string; paths: string[] };

export function effectiveCwd(directories: readonly string[]): string {
  return directories.reduce((cwd, value) => path.resolve(cwd, value), "/");
}

export async function validatedCwd(
  directories: readonly string[],
): Promise<string> {
  let cwd = "/";
  for (const value of directories) {
    const next = path.resolve(cwd, value);
    let metadata;
    try {
      metadata = await stat(next);
    } catch {
      throw new Error(`working directory does not exist: ${next}`);
    }
    if (!metadata.isDirectory()) {
      throw new Error(`working directory is not a directory: ${next}`);
    }
    cwd = next;
  }
  return cwd;
}

export async function repository(
  directories: readonly string[],
): Promise<Repository> {
  const cwd = await validatedCwd(directories);
  const dir = await git.findRoot({ fs, filepath: cwd });
  const gitdir = path.join(dir, ".git");
  const metadata = await lstat(gitdir);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error(
      "only repositories with a regular .git directory are supported",
    );
  }
  return { cwd, dir, gitdir };
}

export function relativePaths(
  repo: Repository,
  values: readonly string[],
): string[] {
  return values.map((value) => {
    if (value.includes("\0"))
      throw new Error("paths must not contain NUL bytes");
    if (value.startsWith(":("))
      throw new Error(`pathspec magic is not supported: ${value}`);
    const absolute = path.resolve(repo.cwd, value);
    const relative =
      path.relative(repo.dir, absolute).split(path.sep).join("/") || ".";
    if (relative === ".." || relative.startsWith("../")) {
      throw new Error(`path is outside the repository: ${value}`);
    }
    return relative;
  });
}

export async function statusEntries(
  repo: Repository,
  paths: readonly string[],
): Promise<StatusEntry[]> {
  const rows = await git.statusMatrix({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    filepaths: paths.length ? relativePaths(repo, paths) : undefined,
    ignored: false,
    refresh: false,
  });
  const [headSnapshots, indexSnapshots] = await Promise.all([
    rows.some(([, head]) => head !== 0)
      ? treeSnapshotMap(repo, "HEAD")
      : new Map<string, Snapshot>(),
    indexSnapshotMap(repo),
  ]);
  const entries = await Promise.all(
    rows.map(async ([filepath, head, worktree, stage]) => {
      const headSnapshot = head ? (headSnapshots.get(filepath) ?? {}) : {};
      const index = stage ? (indexSnapshots.get(filepath) ?? {}) : {};
      const worktreeSnapshotValue = worktree
        ? await worktreeSnapshot(repo, filepath)
        : {};
      assertSnapshotsSupported(filepath, [
        headSnapshot,
        index,
        worktreeSnapshotValue,
      ]);
      const indexChanged = !sameSnapshot(headSnapshot, index);
      const worktreeChanged = !sameSnapshot(index, worktreeSnapshotValue);
      const code =
        head === 0 && stage === 0 && worktree !== 0
          ? "??"
          : `${indexCode(head, stage, indexChanged)}${worktreeCode(stage, worktree, worktreeChanged)}`;
      return {
        path: filepath,
        index: indexState(head, stage, indexChanged),
        worktree: worktreeState(stage, worktree, worktreeChanged),
        code,
      };
    }),
  );
  return entries
    .filter((entry) => entry.code !== "  ")
    .sort((left, right) => left.path.localeCompare(right.path));
}

function indexState(head: number, stage: number, changed: boolean): string {
  if (!changed) return "unmodified";
  if (stage === 0) return "deleted";
  return head === 0 ? "added" : "modified";
}

function worktreeState(
  stage: number,
  worktree: number,
  changed: boolean,
): string {
  if (!changed) return "unmodified";
  if (worktree === 0) return "deleted";
  return stage === 0 ? "untracked" : "modified";
}

function indexCode(head: number, stage: number, changed: boolean): string {
  if (!changed) return " ";
  if (stage === 0) return "D";
  return head === 0 ? "A" : "M";
}

function worktreeCode(
  stage: number,
  worktree: number,
  changed: boolean,
): string {
  if (stage === 0 && worktree !== 0) return "?";
  if (!changed) return " ";
  if (worktree === 0) return "D";
  return "M";
}

export function formatStatus(
  entries: readonly StatusEntry[],
  zeroTerminated: boolean,
): string {
  const separator = zeroTerminated ? "\0" : "\n";
  return entries.length
    ? entries
        .map(
          (entry) =>
            `${entry.code} ${zeroTerminated ? entry.path : quoteGitPath(entry.path)}`,
        )
        .join(separator) +
        separator
    : "";
}

function quoteGitPath(filepath: string): string {
  const bytes = new TextEncoder().encode(filepath);
  if (
    bytes.every(
      (byte) => byte >= 0x20 && byte <= 0x7e && byte !== 0x22 && byte !== 0x5c,
    )
  ) {
    return filepath;
  }

  const escapes = new Map<number, string>([
    [0x07, "\\a"],
    [0x08, "\\b"],
    [0x09, "\\t"],
    [0x0a, "\\n"],
    [0x0b, "\\v"],
    [0x0c, "\\f"],
    [0x0d, "\\r"],
    [0x22, '\\"'],
    [0x5c, "\\\\"],
  ]);
  const quoted = Array.from(bytes, (byte) => {
    const escaped = escapes.get(byte);
    if (escaped) return escaped;
    if (byte >= 0x20 && byte <= 0x7e) return String.fromCharCode(byte);
    return `\\${byte.toString(8).padStart(3, "0")}`;
  }).join("");
  return `"${quoted}"`;
}

export function formatLog(
  entries: readonly LogEntry[],
  oneline: boolean,
): string {
  if (oneline) {
    return entries
      .map(
        (entry) => `${entry.oid.slice(0, 7)} ${entry.message.split("\n")[0]}`,
      )
      .join("\n")
      .concat(entries.length ? "\n" : "");
  }
  return entries
    .map(
      (entry) =>
        `commit ${entry.oid}\nAuthor: ${entry.authorName} <${entry.authorEmail}>\n\n${entry.message
          .trimEnd()
          .split("\n")
          .map((line) => `    ${line}`)
          .join("\n")}\n`,
    )
    .join("\n");
}

type IndexEntry = { path: string; oid: string; mode: number };

async function indexEntries(repo: Repository): Promise<IndexEntry[]> {
  return GitIndexManager.acquire(
    { fs: gitFs, gitdir: repo.gitdir, cache: {}, allowUnmerged: false },
    (index) =>
      index.entries.map(
        (entry: { path: string; oid: string; mode: number }) => ({ ...entry }),
      ),
  );
}

export async function assertSupportedModes(repo: Repository): Promise<void> {
  const unsupported = (await indexEntries(repo)).filter(
    (entry) => entry.mode === 0o100755,
  );
  if (unsupported.length) {
    throw new Error(
      `this runtime cannot preserve executable-file modes across tool invocations: ${unsupported
        .map((entry) => entry.path)
        .join(", ")}`,
    );
  }
}

export async function assertRefSupportedModes(
  repo: Repository,
  ref: string,
): Promise<void> {
  const unsupported = [...(await treeSnapshotMap(repo, ref))]
    .filter(([, snapshot]) => snapshot.mode === 0o100755)
    .map(([filepath]) => filepath);
  if (unsupported.length) {
    throw new Error(
      `this runtime cannot preserve executable-file modes across tool invocations: ${unsupported.join(
        ", ",
      )}`,
    );
  }
}

async function assertSafeWorktreeAncestors(
  repo: Repository,
  filepath: string,
): Promise<void> {
  let ancestor = repo.dir;
  for (const segment of filepath.split("/").slice(0, -1)) {
    ancestor = path.join(ancestor, segment);
    try {
      const metadata = await lstat(ancestor);
      if (metadata.isSymbolicLink()) {
        throw new Error(
          `cannot restore through symbolic-link directory: ${filepath}`,
        );
      }
      if (!metadata.isDirectory()) {
        throw new Error(`cannot restore through non-directory: ${filepath}`);
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") break;
      throw error;
    }
  }
}

export async function restoreIndexPaths(
  repo: Repository,
  paths: readonly string[],
): Promise<string[]> {
  const requested = relativePaths(repo, paths);
  const entries = await indexEntries(repo);
  const snapshots = new Map(
    entries.map((entry) => [entry.path, { oid: entry.oid, mode: entry.mode }]),
  );
  const selected = entries.filter((entry) =>
    requested.some(
      (item) =>
        item === "." ||
        entry.path === item ||
        entry.path.startsWith(`${item}/`),
    ),
  );
  if (selected.length === 0) {
    throw new Error("pathspec did not match any files in the index");
  }
  for (const item of requested) {
    if (
      !selected.some(
        (entry) =>
          item === "." ||
          entry.path === item ||
          entry.path.startsWith(`${item}/`),
      )
    ) {
      throw new Error(`pathspec did not match any files in the index: ${item}`);
    }
  }
  const materialized = new Map<string, Snapshot & { bytes: Uint8Array }>();
  for (const entry of selected) {
    const snapshot = await snapshotWithBytes(repo, snapshots.get(entry.path));
    if (!snapshot.bytes) throw new Error(`missing indexed blob: ${entry.path}`);
    materialized.set(entry.path, { ...snapshot, bytes: snapshot.bytes });
  }
  for (const entry of selected) {
    await assertSafeWorktreeAncestors(repo, entry.path);
    try {
      const metadata = await lstat(path.join(repo.dir, entry.path));
      if (metadata.isDirectory() && !metadata.isSymbolicLink()) {
        throw new Error(
          `cannot replace directory with indexed file: ${entry.path}`,
        );
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }
  for (const entry of selected) {
    const absolute = path.join(repo.dir, entry.path);
    try {
      await fs.promises.rm(absolute, { force: true });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    await fs.promises.mkdir(path.dirname(absolute), { recursive: true });
    const snapshot = materialized.get(entry.path)!;
    if (entry.mode === 0o120000) {
      await fs.promises.symlink(text(snapshot.bytes), absolute);
    } else {
      await fs.promises.writeFile(absolute, snapshot.bytes);
    }
  }
  return selected.map((entry) => entry.path).sort();
}

export async function stage(
  repo: Repository,
  paths: readonly string[],
  all: boolean,
  update: boolean,
): Promise<string[]> {
  await assertSupportedModes(repo);
  const requested = paths.length ? relativePaths(repo, paths) : [];
  if (!all && !update && requested.length === 0)
    throw new Error("nothing specified, nothing added");
  const rows = await git.statusMatrix({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    filepaths: requested.length ? requested : undefined,
    ignored: true,
    refresh: false,
  });
  const indexSnapshots = await indexSnapshotMap(repo);
  for (const item of requested) {
    const matchingRows = rows.filter(
      ([filepath, , worktree, staged]) =>
        (worktree !== 0 || staged !== 0) &&
        (item === "." || filepath === item || filepath.startsWith(`${item}/`)),
    );
    if (matchingRows.length === 0) {
      throw new Error(`pathspec did not match any files: ${item}`);
    }
    const selectsTrackedPath = matchingRows.some(
      ([, , , staged]) => staged !== 0,
    );
    if (
      !selectsTrackedPath &&
      item !== "." &&
      (await git.isIgnored({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        filepath: item,
      }))
    ) {
      throw new Error(`the following path is ignored: ${item}`);
    }
  }
  const actions: { path: string; remove: boolean }[] = [];
  for (const [filepath, , worktree, staged] of rows) {
    if (filepath === ".git" || filepath.startsWith(".git/")) continue;
    const tracked = staged !== 0;
    if (update && !tracked) continue;
    if (!all && !update && requested.length && worktree === 0 && !tracked)
      continue;
    if (
      worktree !== 0 &&
      !tracked &&
      (await git.isIgnored({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        filepath,
      }))
    ) {
      continue;
    }
    if (worktree === 0) {
      if (tracked) actions.push({ path: filepath, remove: true });
    } else {
      const metadata = await lstat(path.join(repo.dir, filepath));
      if (!metadata.isSymbolicLink() && (metadata.mode & 0o111) !== 0) {
        throw new Error(
          `this runtime cannot preserve executable-file modes across tool invocations: ${filepath}`,
        );
      }
      if (
        !sameSnapshot(
          staged ? (indexSnapshots.get(filepath) ?? {}) : {},
          await worktreeSnapshot(repo, filepath),
        )
      ) {
        actions.push({ path: filepath, remove: false });
      }
    }
  }
  for (const action of actions) {
    if (action.remove) {
      await git.remove({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        filepath: action.path,
      });
    } else {
      await git.add({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        filepath: action.path,
      });
    }
  }
  return actions.map((action) => action.path).sort();
}

export function parseIdentity(value: string): { name: string; email: string } {
  const match = /^(.*\S)\s+<([^<>\s]+)>$/.exec(value);
  if (!match?.[1] || !match[2])
    throw new Error("author must have the form 'Name <email>'");
  return { name: match[1], email: match[2] };
}

export function commitMessage(parts: readonly string[]): string {
  const message = parts.join("\n\n");
  if (!message.trim()) throw new Error("commit messages must not be empty");
  return message;
}

export async function localIdentity(
  repo: Repository,
): Promise<{ name: string; email: string }> {
  const [name, email] = await Promise.all([
    git.getConfig({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      path: "user.name",
    }),
    git.getConfig({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      path: "user.email",
    }),
  ]);
  if (!name || !email)
    throw new Error(
      "author identity unknown; configure user.name and user.email locally",
    );
  return { name, email };
}

export async function localConfig(
  repo: Repository,
  key: string,
  value: string | undefined,
): Promise<string> {
  if (key !== "user.name" && key !== "user.email")
    throw new Error("only user.name and user.email are supported");
  if (value !== undefined) {
    if (
      value.trim() !== value ||
      value.includes('"') ||
      /[\0\r\n\u2028\u2029]/.test(value) ||
      (/[#;]/.test(value) && value.endsWith("\\"))
    ) {
      throw new Error(
        "configuration value cannot be represented safely in the local Git config",
      );
    }
    await git.setConfig({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      path: key,
      value,
    });
  }
  const configured = await git.getConfig({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    path: key,
  });
  if (configured === undefined)
    throw new Error(`local configuration is not set: ${key}`);
  return configured;
}

export async function createCommit(
  repo: Repository,
  messages: readonly string[],
  author: string | undefined,
  allowEmpty: boolean,
): Promise<string> {
  await assertSupportedModes(repo);
  const committer = await localIdentity(repo);
  return git.commit({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    message: commitMessage(messages),
    author: author ? parseIdentity(author) : committer,
    committer,
    noUpdateBranch: false,
    dryRun: false,
    disallowEmpty: !allowEmpty,
  });
}

export async function branchCommand(
  repo: Repository,
  shouldDelete: boolean,
  name: string | undefined,
  startPoint: string | undefined,
): Promise<BranchEntry[]> {
  await assertSupportedModes(repo);
  const current = await git.currentBranch({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    fullname: false,
  });
  if (shouldDelete) {
    if (!name || startPoint)
      throw new Error("branch -d requires exactly one branch name");
    if (name === current)
      throw new Error(`cannot delete branch '${name}' checked out at '${repo.dir}'`);
    const names = await git.listBranches({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
    });
    if (!names.includes(name)) throw new Error(`branch '${name}' not found`);
    const ref = localBranchRef(name);
    const branchOid = await resolveDocumentedRef(repo, ref);
    const headOid = await resolveDocumentedRef(repo, "HEAD");
    if (
      branchOid !== headOid &&
      !(await git.isDescendent({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        oid: headOid,
        ancestor: branchOid,
      }))
    ) {
      throw new Error(`branch '${name}' is not fully merged`);
    }
    await git.deleteBranch({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ref,
    });
  } else if (name) {
    const object = startPoint
      ? await resolveCommitRef(repo, startPoint)
      : undefined;
    await git.branch({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ref: name,
      object,
    });
  }
  const names = await git.listBranches({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
  });
  return Promise.all(
    names.sort().map(async (branch) => ({
      name: branch,
      current: branch === current,
      oid: await git.resolveRef({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        ref: `refs/heads/${branch}`,
      }),
    })),
  );
}

function localBranchRef(name: string): string {
  return `refs/heads/${name}`;
}

async function resolveCommitRef(
  repo: Repository,
  ref: string,
): Promise<string> {
  let oid = await resolveDocumentedRef(repo, ref);
  for (let depth = 0; depth < 16; depth += 1) {
    try {
      const object = await git.readObject({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        oid,
        format: "parsed",
      });
      if (object.type === "commit") return oid;
      if (object.type !== "tag") break;
      oid = (object.object as TagObject).object;
    } catch {
      break;
    }
  }
  throw new Error(`branch start point is not an existing commit: ${ref}`);
}

async function materializeTree(repo: Repository, ref: string): Promise<void> {
  const snapshots = await treeSnapshotMap(repo, ref);
  for (const [filepath, snapshot] of snapshots) {
    assertSnapshotsSupported(filepath, [snapshot]);
    await snapshotWithBytes(repo, snapshot);
  }
}

type PreservedCheckoutState = {
  index?: IndexEntry;
  worktree: Snapshot;
};

type CheckoutWorktreeState = {
  paths: Map<string, Snapshot>;
  absentParents: string[];
};

function hasFileAncestor(
  snapshots: ReadonlyMap<string, Snapshot>,
  filepath: string,
): boolean {
  let ancestor = "";
  for (const segment of filepath.split("/").slice(0, -1)) {
    ancestor = ancestor ? `${ancestor}/${segment}` : segment;
    if (snapshots.has(ancestor)) return true;
  }
  return false;
}

function assertNoStructuralTreeTransition(
  headSnapshots: ReadonlyMap<string, Snapshot>,
  targetSnapshots: ReadonlyMap<string, Snapshot>,
): void {
  if (
    [...headSnapshots.keys()].some((filepath) =>
      hasFileAncestor(targetSnapshots, filepath),
    ) ||
    [...targetSnapshots.keys()].some((filepath) =>
      hasFileAncestor(headSnapshots, filepath),
    )
  ) {
    throw new Error(
      "checkout between file and directory trees is not supported safely",
    );
  }
}

async function preservedCheckoutState(
  repo: Repository,
  targetRef: string,
): Promise<Map<string, PreservedCheckoutState>> {
  const [headSnapshots, targetSnapshots, entries, rows] = await Promise.all([
    treeSnapshotMap(repo, "HEAD"),
    treeSnapshotMap(repo, targetRef),
    indexEntries(repo),
    git.statusMatrix({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ignored: false,
      refresh: false,
    }),
  ]);
  assertNoStructuralTreeTransition(headSnapshots, targetSnapshots);
  const indexEntriesByPath = new Map(entries.map((entry) => [entry.path, entry]));
  const indexSnapshots = new Map(
    entries.map((entry) => [
      entry.path,
      { oid: entry.oid, mode: entry.mode } satisfies Snapshot,
    ]),
  );
  const candidates = new Set([
    ...headSnapshots.keys(),
    ...indexSnapshots.keys(),
    ...targetSnapshots.keys(),
    ...rows.map(([filepath]) => filepath),
  ]);
  const preserved = new Map<string, PreservedCheckoutState>();
  for (const filepath of candidates) {
    const head = headSnapshots.get(filepath) ?? {};
    const index = indexSnapshots.get(filepath) ?? {};
    const target = targetSnapshots.get(filepath) ?? {};
    const worktree = await worktreeSnapshot(repo, filepath);
    assertSnapshotsSupported(filepath, [head, index, target, worktree]);
    let worktreeDirectory = false;
    try {
      const metadata = await lstat(path.join(repo.dir, filepath));
      worktreeDirectory =
        metadata.isDirectory() && !metadata.isSymbolicLink();
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    const trackedDirectory = [...headSnapshots.keys(), ...indexSnapshots.keys()]
      .some((candidate) => candidate.startsWith(`${filepath}/`));
    const locallyChanged =
      (worktreeDirectory && !trackedDirectory) ||
      !sameSnapshot(head, index) ||
      !sameSnapshot(index, worktree);
    if (!locallyChanged) continue;
    if (!sameSnapshot(head, target)) {
      throw new Error(
        `local changes would be overwritten by checkout: ${filepath}`,
      );
    }
    if (worktreeDirectory) {
      throw new Error(
        `local directory replacement cannot be preserved by checkout: ${filepath}`,
      );
    }
    let ancestor = "";
    for (const segment of filepath.split("/").slice(0, -1)) {
      ancestor = ancestor ? `${ancestor}/${segment}` : segment;
      if (targetSnapshots.has(ancestor)) {
        throw new Error(
          `local changes would be overwritten by checkout: ${filepath}`,
        );
      }
    }
    await assertSafeWorktreeAncestors(repo, filepath);
    preserved.set(filepath, {
      index: indexEntriesByPath.get(filepath),
      worktree,
    });
  }
  return preserved;
}

async function captureCheckoutWorktree(
  repo: Repository,
  targetRef: string,
  preserved: ReadonlyMap<string, PreservedCheckoutState>,
): Promise<CheckoutWorktreeState> {
  const [headSnapshots, targetSnapshots] = await Promise.all([
    treeSnapshotMap(repo, "HEAD"),
    treeSnapshotMap(repo, targetRef),
  ]);
  const paths = new Set([...headSnapshots.keys(), ...targetSnapshots.keys()]);
  for (const filepath of [...paths]) {
    if (
      sameSnapshot(
        headSnapshots.get(filepath) ?? {},
        targetSnapshots.get(filepath) ?? {},
      ) &&
      !preserved.has(filepath)
    ) {
      paths.delete(filepath);
    }
  }
  for (const filepath of preserved.keys()) paths.add(filepath);
  const snapshots = new Map<string, Snapshot>();
  const absentParents = new Set<string>();
  for (const filepath of paths) {
    await assertSafeWorktreeAncestors(repo, filepath);
    snapshots.set(filepath, await worktreeSnapshot(repo, filepath));
    const segments = filepath.split("/").slice(0, -1);
    let parent = repo.dir;
    let missing = false;
    for (const segment of segments) {
      parent = path.join(parent, segment);
      if (missing) {
        absentParents.add(parent);
        continue;
      }
      try {
        await lstat(parent);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
        missing = true;
        absentParents.add(parent);
      }
    }
  }
  return {
    paths: snapshots,
    absentParents: [...absentParents].sort(
      (left, right) => right.length - left.length,
    ),
  };
}

async function removeSymlinkLeaves(
  repo: Repository,
  state: CheckoutWorktreeState,
): Promise<void> {
  for (const filepath of state.paths.keys()) {
    const absolute = path.join(repo.dir, filepath);
    try {
      if ((await lstat(absolute)).isSymbolicLink()) {
        await fs.promises.rm(absolute);
      }
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== "ENOENT" && code !== "ENOTDIR") throw error;
    }
  }
}

async function restoreCheckoutWorktree(
  repo: Repository,
  state: CheckoutWorktreeState,
): Promise<void> {
  const entries = [...state.paths].sort(
    ([left], [right]) => right.length - left.length,
  );
  for (const [filepath] of entries) {
    try {
      await fs.promises.rm(path.join(repo.dir, filepath), { force: true });
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== "ENOENT" && code !== "ENOTDIR" && code !== "EISDIR")
        throw error;
    }
  }
  for (const [filepath, snapshot] of entries.reverse()) {
    if (!snapshot.bytes || !snapshot.mode) continue;
    const absolute = path.join(repo.dir, filepath);
    await fs.promises.mkdir(path.dirname(absolute), { recursive: true });
    if (snapshot.mode === 0o120000) {
      await fs.promises.symlink(text(snapshot.bytes), absolute);
    } else {
      await fs.promises.writeFile(absolute, snapshot.bytes);
    }
  }
  for (const parent of state.absentParents) {
    try {
      await fs.promises.rmdir(parent);
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== "ENOENT" && code !== "ENOTEMPTY") throw error;
    }
  }
}

async function restoreCheckoutState(
  repo: Repository,
  preserved: ReadonlyMap<string, PreservedCheckoutState>,
): Promise<void> {
  await GitIndexManager.acquire(
    { fs: gitFs, gitdir: repo.gitdir, cache: {}, allowUnmerged: false },
    (index) => {
      for (const [filepath, state] of preserved) {
        if (state.index) {
          index.insert({
            filepath,
            stats: state.index,
            oid: state.index.oid,
          });
        } else {
          index.delete({ filepath });
        }
      }
    },
  );
  for (const [filepath, state] of preserved) {
    const absolute = path.join(repo.dir, filepath);
    await fs.promises.rm(absolute, { force: true });
    if (!state.worktree.bytes || !state.worktree.mode) continue;
    await fs.promises.mkdir(path.dirname(absolute), { recursive: true });
    if (state.worktree.mode === 0o120000) {
      await fs.promises.symlink(text(state.worktree.bytes), absolute);
    } else {
      await fs.promises.writeFile(absolute, state.worktree.bytes);
    }
  }
}

async function localBranchForRef(
  repo: Repository,
  ref: string,
): Promise<string | undefined> {
  const names = await git.listBranches({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
  });
  if (ref.startsWith("refs/heads/")) {
    const name = ref.slice("refs/heads/".length);
    return names.includes(name) ? name : undefined;
  }
  return names.includes(ref) ? ref : undefined;
}

async function rollbackCheckout(
  repo: Repository,
  originalOid: string,
  originalHead: string,
  preserved: ReadonlyMap<string, PreservedCheckoutState>,
  worktree: CheckoutWorktreeState,
): Promise<void> {
  await removeSymlinkLeaves(repo, worktree);
  await git.checkout({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    ref: originalOid,
    force: true,
  });
  await fs.promises.writeFile(path.join(repo.gitdir, "HEAD"), originalHead);
  await restoreCheckoutState(repo, preserved);
  await restoreCheckoutWorktree(repo, worktree);
}

export async function checkoutCommand(
  repo: Repository,
  newBranch: string | undefined,
  detach: boolean,
  ref: string | undefined,
  paths: readonly string[],
): Promise<MutationResult> {
  await assertSupportedModes(repo);
  if (paths.length) {
    if (newBranch || detach || ref)
      throw new Error("path checkout cannot be combined with branch switching");
    const filepaths = await restoreIndexPaths(repo, paths);
    return {
      summary: `restored ${filepaths.length} path(s) from the index`,
      paths: filepaths,
    };
  }
  if (newBranch) {
    if (detach) throw new Error("-b and --detach are mutually exclusive");
    const start = ref
      ? await resolveCommitRef(repo, ref)
      : await resolveCommitRef(repo, "HEAD");
    await assertRefSupportedModes(repo, start);
    const preserved = await preservedCheckoutState(repo, start);
    const worktree = await captureCheckoutWorktree(repo, start, preserved);
    const originalOid = await resolveDocumentedRef(repo, "HEAD");
    const originalHead = await readFile(path.join(repo.gitdir, "HEAD"), "utf8");
    await Promise.all([
      materializeTree(repo, start),
      materializeTree(repo, originalOid),
    ]);
    await git.branch({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ref: newBranch,
      object: start,
    });
    try {
      await removeSymlinkLeaves(repo, worktree);
      await git.checkout({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        ref: localBranchRef(newBranch),
        force: true,
      });
      await restoreCheckoutState(repo, preserved);
    } catch (error) {
      await rollbackCheckout(
        repo,
        originalOid,
        originalHead,
        preserved,
        worktree,
      );
      await git.deleteBranch({
        fs,
        dir: repo.dir,
        gitdir: repo.gitdir,
        ref: localBranchRef(newBranch),
      });
      throw error;
    }
    return {
      summary: `switched to a new branch '${newBranch}'`,
      paths: [],
    };
  }
  if (!ref)
    throw new Error(
      "checkout requires a branch, --detach <ref>, or -- <paths>",
    );
  const localBranch = detach ? undefined : await localBranchForRef(repo, ref);
  const checkoutRef = localBranch ? localBranchRef(localBranch) : ref;
  const targetOid = await resolveCommitRef(repo, checkoutRef);
  await assertRefSupportedModes(repo, targetOid);
  const preserved = await preservedCheckoutState(repo, targetOid);
  const worktree = await captureCheckoutWorktree(repo, targetOid, preserved);
  const originalOid = await resolveDocumentedRef(repo, "HEAD");
  const originalHead = await readFile(path.join(repo.gitdir, "HEAD"), "utf8");
  await Promise.all([
    materializeTree(repo, targetOid),
    materializeTree(repo, originalOid),
  ]);
  try {
    await removeSymlinkLeaves(repo, worktree);
    await git.checkout({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ref: localBranch ? localBranchRef(localBranch) : targetOid,
      force: true,
    });
    await restoreCheckoutState(repo, preserved);
  } catch (error) {
    await rollbackCheckout(
      repo,
      originalOid,
      originalHead,
      preserved,
      worktree,
    );
    throw error;
  }
  if (detach) {
    await fs.promises.writeFile(path.join(repo.gitdir, "HEAD"), `${targetOid}\n`);
  }
  const current = await git.currentBranch({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    fullname: false,
  });
  return {
    summary:
      detach || !localBranch || !current
        ? `HEAD is now at ${ref}`
        : `switched to branch '${current}'`,
    paths: [],
  };
}

export async function resolveDocumentedRef(
  repo: Repository,
  ref: string,
): Promise<string> {
  if (
    !/^(HEAD|[0-9a-f]{40}|[A-Za-z0-9][A-Za-z0-9._\/-]*)$/.test(ref) ||
    ref.includes("..")
  ) {
    throw new Error(`unsupported revision expression: ${ref}`);
  }
  return git.resolveRef({ fs, dir: repo.dir, gitdir: repo.gitdir, ref });
}

type Snapshot = { oid?: string; mode?: number; bytes?: Uint8Array };

async function indexSnapshotMap(
  repo: Repository,
): Promise<Map<string, Snapshot>> {
  return new Map(
    (await indexEntries(repo)).map((entry) => [
      entry.path,
      { oid: entry.oid, mode: entry.mode },
    ]),
  );
}

async function snapshotWithBytes(
  repo: Repository,
  snapshot: Snapshot | undefined,
): Promise<Snapshot> {
  if (!snapshot?.oid) return {};
  const object = await git.readBlob({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    oid: snapshot.oid,
  });
  return { ...snapshot, bytes: object.blob };
}

async function worktreeSnapshot(
  repo: Repository,
  filepath: string,
): Promise<Snapshot> {
  let ancestor = repo.dir;
  for (const segment of filepath.split("/").slice(0, -1)) {
    ancestor = path.join(ancestor, segment);
    try {
      const metadata = await lstat(ancestor);
      if (metadata.isSymbolicLink() || !metadata.isDirectory()) return {};
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === "ENOENT" || code === "ENOTDIR") return {};
      throw error;
    }
  }
  const absolute = path.join(repo.dir, filepath);
  try {
    const stats = await lstat(absolute);
    if (stats.isSymbolicLink()) {
      const bytes = new TextEncoder().encode(await readlink(absolute));
      return {
        mode: 0o120000,
        oid: (await git.hashBlob({ object: bytes })).oid,
        bytes,
      };
    }
    if (!stats.isFile()) return {};
    const bytes = await readFile(absolute);
    return {
      mode: (stats.mode & 0o111) === 0 ? 0o100644 : 0o100755,
      oid: (await git.hashBlob({ object: bytes })).oid,
      bytes,
    };
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT" || code === "ENOTDIR") return {};
    throw error;
  }
}

function sameSnapshot(left: Snapshot, right: Snapshot): boolean {
  return left.oid === right.oid && left.mode === right.mode;
}

async function treeSnapshotMap(
  repo: Repository,
  ref: string,
): Promise<Map<string, Snapshot>> {
  const oid = await resolveDocumentedRef(repo, ref);
  const entries = await git.walk({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    trees: [git.TREE({ ref: oid })],
    map: async (entryPath, [entry]) => {
      if (!entry || entryPath === ".") return undefined;
      const mode = await entry.mode();
      if (mode === 0o040000) return undefined;
      return [
        entryPath,
        {
          oid: await entry.oid(),
          mode,
        } satisfies Snapshot,
      ] as const;
    },
  });
  return new Map(entries);
}

function binary(bytes: Uint8Array | undefined): boolean {
  if (!bytes) return false;
  if (bytes.includes(0)) return true;
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return false;
  } catch {
    return true;
  }
}

function text(bytes: Uint8Array | undefined): string {
  return bytes
    ? new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes)
    : "";
}

function assertSnapshotsSupported(
  filepath: string,
  snapshots: readonly Snapshot[],
): void {
  if (snapshots.some((snapshot) => snapshot.mode === 0o100755)) {
    throw new Error(
      `this runtime cannot preserve executable-file modes across tool invocations: ${filepath}`,
    );
  }
}

export function limitedDiffOutput(value: string): string {
  if (new TextEncoder().encode(value).byteLength > MAX_DIFF_BYTES) {
    throw new Error(`diff exceeds the ${MAX_DIFF_BYTES}-byte output limit`);
  }
  return value;
}

export async function diff(
  repo: Repository,
  refs: readonly string[],
  paths: readonly string[],
  cached: boolean,
  nameOnly: boolean,
  context: number,
  statOnly = false,
): Promise<{
  patch: string;
  stat: string;
  paths: string[];
  hasChanges: boolean;
}> {
  await assertSupportedModes(repo);
  if (cached && refs.length > 1)
    throw new Error("--cached accepts at most one revision");
  let leftRef = refs[0];
  let emptyLeft = false;
  if (!leftRef && cached) {
    try {
      leftRef = await resolveDocumentedRef(repo, "HEAD");
    } catch (error) {
      if ((error as { code?: string }).code !== "NotFoundError") throw error;
      emptyLeft = true;
    }
  }
  const rightRef = refs[1];
  const indexSnapshots = await indexSnapshotMap(repo);
  const leftSnapshots = emptyLeft
    ? new Map<string, Snapshot>()
    : leftRef
      ? await treeSnapshotMap(repo, leftRef)
      : indexSnapshots;
  const rightSnapshots = rightRef
    ? await treeSnapshotMap(repo, rightRef)
    : cached
      ? indexSnapshots
      : undefined;
  const candidates = new Set(leftSnapshots.keys());
  if (rightSnapshots) {
    for (const filepath of rightSnapshots.keys()) candidates.add(filepath);
  } else {
    for (const filepath of indexSnapshots.keys()) candidates.add(filepath);
  }
  const selected = paths.length ? relativePaths(repo, paths) : [];
  const changed: string[] = [];
  const stats: { path: string; insertions: number; deletions: number }[] = [];
  let patch = "";
  for (const filepath of [...candidates].sort()) {
    if (
      selected.length &&
      !selected.some(
        (item) =>
          item === "." || filepath === item || filepath.startsWith(`${item}/`),
      )
    )
      continue;
    const leftMetadata = leftSnapshots.get(filepath) ?? {};
    const rightMetadata = rightSnapshots?.get(filepath);
    const right = rightSnapshots
      ? await snapshotWithBytes(repo, rightMetadata)
      : await worktreeSnapshot(repo, filepath);
    assertSnapshotsSupported(filepath, [leftMetadata, right]);
    if (sameSnapshot(leftMetadata, right)) continue;
    const left = await snapshotWithBytes(repo, leftMetadata);
    if (!left.bytes && !right.bytes) continue;
    changed.push(filepath);
    if (nameOnly) continue;
    const leftPath = left.bytes ? quoteGitPath(`a/${filepath}`) : "/dev/null";
    const rightPath = right.bytes ? quoteGitPath(`b/${filepath}`) : "/dev/null";
    const header = `diff --git ${quoteGitPath(`a/${filepath}`)} ${quoteGitPath(`b/${filepath}`)}\n`;
    const modeChange =
      left.mode === right.mode
        ? ""
        : !left.mode
          ? `new file mode ${right.mode?.toString(8)}\n`
          : !right.mode
            ? `deleted file mode ${left.mode.toString(8)}\n`
            : `old mode ${left.mode.toString(8)}\nnew mode ${right.mode.toString(8)}\n`;
    if (
      left.bytes &&
      right.bytes &&
      Buffer.compare(left.bytes, right.bytes) === 0
    ) {
      patch += header + modeChange;
      stats.push({ path: filepath, insertions: 0, deletions: 0 });
      continue;
    }
    let filePatch: string;
    if (binary(left.bytes) || binary(right.bytes)) {
      filePatch = `${header}${modeChange}Binary files ${leftPath} and ${rightPath} differ\n`;
      stats.push({ path: filepath, insertions: 0, deletions: 0 });
    } else {
      const inputBytes =
        (left.bytes?.byteLength ?? 0) + (right.bytes?.byteLength ?? 0);
      if (inputBytes > MAX_DIFF_INPUT_BYTES) {
        throw new Error(
          `diff input exceeds the ${MAX_DIFF_INPUT_BYTES}-byte computation limit: ${filepath}`,
        );
      }
      const leftText = text(left.bytes);
      const rightText = text(right.bytes);
      const inputLines =
        leftText.split("\n").length + rightText.split("\n").length;
      if (inputLines > MAX_DIFF_LINES) {
        throw new Error(
          `diff input exceeds the ${MAX_DIFF_LINES}-line computation limit: ${filepath}`,
        );
      }
      const structured = structuredPatch(
        leftPath,
        rightPath,
        leftText,
        rightText,
        undefined,
        undefined,
        { context, maxEditLength: MAX_DIFF_EDIT_LENGTH },
      );
      if (!structured) {
        throw new Error(
          `diff computation exceeds the ${MAX_DIFF_EDIT_LENGTH}-edit limit: ${filepath}`,
        );
      }
      const lines = structured.hunks.flatMap((hunk) => hunk.lines);
      stats.push({
        path: filepath,
        insertions: lines.filter((line) => line.startsWith("+")).length,
        deletions: lines.filter((line) => line.startsWith("-")).length,
      });
      filePatch = statOnly
        ? ""
        : header +
          modeChange +
          formatPatch(structured).replace(/^=+\n/, "");
    }
    if (!statOnly) {
      patch += filePatch;
      limitedDiffOutput(patch);
    }
  }
  const stat = formatDiffStat(stats);
  const output = statOnly
    ? stat
    : nameOnly
      ? changed.map(quoteGitPath).join("\n") + (changed.length ? "\n" : "")
      : patch;
  return {
    patch: limitedDiffOutput(output),
    stat: limitedDiffOutput(stat),
    paths: changed,
    hasChanges: changed.length > 0,
  };
}

function formatDiffStat(
  stats: readonly { path: string; insertions: number; deletions: number }[],
): string {
  if (stats.length === 0) return "";
  const lines = stats.map(({ path, insertions, deletions }) => {
    const total = insertions + deletions;
    return ` ${quoteGitPath(path)} | ${total} ${"+".repeat(insertions)}${"-".repeat(deletions)}`;
  });
  const insertions = stats.reduce(
    (total, entry) => total + entry.insertions,
    0,
  );
  const deletions = stats.reduce((total, entry) => total + entry.deletions, 0);
  const files = `${stats.length} file${stats.length === 1 ? "" : "s"} changed`;
  const summary = [
    files,
    insertions
      ? `${insertions} insertion${insertions === 1 ? "" : "s"}(+)`
      : undefined,
    deletions
      ? `${deletions} deletion${deletions === 1 ? "" : "s"}(-)`
      : undefined,
  ]
    .filter((item): item is string => item !== undefined)
    .join(", ");
  return `${lines.join("\n")}\n ${summary}\n`;
}
