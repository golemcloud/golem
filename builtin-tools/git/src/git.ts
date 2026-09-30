import fs from "node:fs";
import { lstat, readFile } from "node:fs/promises";
import path from "node:path";
import * as git from "isomorphic-git";
import { GitIndexManager } from "isomorphic-git/managers";
import { FileSystem } from "isomorphic-git/models";
import { createTwoFilesPatch } from "diff";

export const MAX_DIFF_BYTES = 2 * 1024 * 1024;
const gitFs = new FileSystem(fs);

export type Repository = { cwd: string; dir: string; gitdir: string };
export type StatusEntry = {
  path: string;
  index: string;
  worktree: string;
  code: string;
};

export function effectiveCwd(directories: readonly string[]): string {
  return directories.reduce((cwd, value) => path.resolve(cwd, value), "/");
}

export async function repository(
  directories: readonly string[],
): Promise<Repository> {
  const cwd = effectiveCwd(directories);
  const dir = await git.findRoot({ fs, filepath: cwd });
  return { cwd, dir, gitdir: path.join(dir, ".git") };
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
  return rows
    .map(([filepath, head, worktree, stage]) => ({
      path: filepath,
      index: indexState(head, stage),
      worktree: worktreeState(stage, worktree),
      code: `${indexCode(head, stage)}${worktreeCode(stage, worktree)}`,
    }))
    .filter((entry) => entry.code !== "  ")
    .sort((left, right) => left.path.localeCompare(right.path));
}

function indexState(head: number, stage: number): string {
  if (head === stage) return "unmodified";
  if (stage === 0) return "deleted";
  return head === 0 ? "added" : "modified";
}

function worktreeState(stage: number, worktree: number): string {
  if (stage === worktree) return "unmodified";
  if (worktree === 0) return "deleted";
  return stage === 0 ? "untracked" : "modified";
}

function indexCode(head: number, stage: number): string {
  if (head === stage) return " ";
  if (stage === 0) return "D";
  return head === 0 ? "A" : "M";
}

function worktreeCode(stage: number, worktree: number): string {
  if (stage === 0 && worktree !== 0) return "?";
  if (stage === worktree) return " ";
  if (worktree === 0) return "D";
  return "M";
}

export function formatStatus(
  entries: readonly StatusEntry[],
  zeroTerminated: boolean,
): string {
  const separator = zeroTerminated ? "\0" : "\n";
  return entries.length
    ? entries.map((entry) => `${entry.code} ${entry.path}`).join(separator) +
        separator
    : "";
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
    (entry) => entry.mode === 0o100755 || entry.mode === 0o120000,
  );
  if (unsupported.length) {
    throw new Error(
      `this runtime cannot preserve executable-file or symlink modes across tool invocations: ${unsupported
        .map((entry) => entry.path)
        .join(", ")}`,
    );
  }
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
  const changed: string[] = [];
  for (const [filepath, head, worktree, staged] of rows) {
    const tracked = head !== 0 || staged !== 0;
    if (update && !tracked) continue;
    if (!all && !update && requested.length && worktree === 0 && !tracked)
      continue;
    if (worktree === 0) {
      if (tracked) {
        await git.remove({ fs, dir: repo.dir, gitdir: repo.gitdir, filepath });
        changed.push(filepath);
      }
    } else if (worktree !== staged) {
      if (
        !tracked &&
        (await git.isIgnored({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          filepath,
        }))
      ) {
        if (
          requested.some(
            (item) => filepath === item || filepath.startsWith(`${item}/`),
          )
        ) {
          throw new Error(`the following path is ignored: ${filepath}`);
        }
        continue;
      }
      await git.add({ fs, dir: repo.dir, gitdir: repo.gitdir, filepath });
      changed.push(filepath);
    }
  }
  return changed.sort();
}

export function parseIdentity(value: string): { name: string; email: string } {
  const match = /^(.*\S)\s+<([^<>\s]+)>$/.exec(value);
  if (!match?.[1] || !match[2])
    throw new Error("author must have the form 'Name <email>'");
  return { name: match[1], email: match[2] };
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

async function indexSnapshot(
  repo: Repository,
  filepath: string,
): Promise<Snapshot> {
  const entry = (await indexEntries(repo)).find(
    (candidate) => candidate.path === filepath,
  );
  if (!entry) return {};
  const object = await git.readBlob({
    fs,
    dir: repo.dir,
    gitdir: repo.gitdir,
    oid: entry.oid,
  });
  return { oid: entry.oid, mode: entry.mode, bytes: object.blob };
}

async function worktreeSnapshot(
  repo: Repository,
  filepath: string,
): Promise<Snapshot> {
  const absolute = path.join(repo.dir, filepath);
  try {
    const stats = await lstat(absolute);
    if (stats.isSymbolicLink())
      throw new Error(
        `symbolic links are unsupported by this runtime: ${filepath}`,
      );
    if (!stats.isFile()) return {};
    return { mode: 0o100644, bytes: await readFile(absolute) };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
}

async function treeSnapshot(
  repo: Repository,
  ref: string,
  filepath: string,
): Promise<Snapshot> {
  const oid = await resolveDocumentedRef(repo, ref);
  try {
    const result = await git.readBlob({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      oid,
      filepath,
    });
    return { oid: result.oid, mode: 0o100644, bytes: result.blob };
  } catch (error) {
    if ((error as { code?: string }).code === "NotFoundError") return {};
    throw error;
  }
}

async function snapshotPaths(
  repo: Repository,
  ref?: string,
): Promise<string[]> {
  if (ref)
    return git.listFiles({
      fs,
      dir: repo.dir,
      gitdir: repo.gitdir,
      ref: await resolveDocumentedRef(repo, ref),
    });
  return (await indexEntries(repo)).map((entry) => entry.path);
}

function binary(bytes: Uint8Array | undefined): boolean {
  return bytes?.includes(0) ?? false;
}

function text(bytes: Uint8Array | undefined): string {
  return bytes ? new TextDecoder("utf-8", { fatal: true }).decode(bytes) : "";
}

export async function diff(
  repo: Repository,
  refs: readonly string[],
  paths: readonly string[],
  cached: boolean,
  nameOnly: boolean,
  context: number,
): Promise<{ patch: string; paths: string[]; hasChanges: boolean }> {
  await assertSupportedModes(repo);
  if (cached && refs.length > 1)
    throw new Error("--cached accepts at most one revision");
  const leftRef = refs[0] ?? (cached ? "HEAD" : undefined);
  const rightRef = refs[1];
  const candidates = new Set<string>();
  for (const item of await snapshotPaths(repo, leftRef)) candidates.add(item);
  if (rightRef)
    for (const item of await snapshotPaths(repo, rightRef))
      candidates.add(item);
  else for (const item of await snapshotPaths(repo)) candidates.add(item);
  const selected = paths.length ? relativePaths(repo, paths) : [];
  const changed: string[] = [];
  let patch = "";
  for (const filepath of [...candidates].sort()) {
    if (
      selected.length &&
      !selected.some(
        (item) => filepath === item || filepath.startsWith(`${item}/`),
      )
    )
      continue;
    const left = leftRef
      ? await treeSnapshot(repo, leftRef, filepath)
      : await indexSnapshot(repo, filepath);
    const right = rightRef
      ? await treeSnapshot(repo, rightRef, filepath)
      : cached
        ? await indexSnapshot(repo, filepath)
        : await worktreeSnapshot(repo, filepath);
    if (left.oid && left.oid === right.oid) continue;
    if (
      left.bytes &&
      right.bytes &&
      Buffer.compare(left.bytes, right.bytes) === 0 &&
      left.mode === right.mode
    )
      continue;
    if (!left.bytes && !right.bytes) continue;
    changed.push(filepath);
    if (nameOnly) continue;
    const header = `diff --git a/${filepath} b/${filepath}\n`;
    if (binary(left.bytes) || binary(right.bytes)) {
      patch += `${header}Binary files a/${filepath} and b/${filepath} differ\n`;
    } else {
      patch +=
        header +
        createTwoFilesPatch(
          `a/${filepath}`,
          `b/${filepath}`,
          text(left.bytes),
          text(right.bytes),
          undefined,
          undefined,
          { context },
        );
    }
    if (new TextEncoder().encode(patch).byteLength > MAX_DIFF_BYTES) {
      throw new Error(`diff exceeds the ${MAX_DIFF_BYTES}-byte output limit`);
    }
  }
  return {
    patch: nameOnly ? changed.join("\n") + (changed.length ? "\n" : "") : patch,
    paths: changed,
    hasChanges: changed.length > 0,
  };
}
