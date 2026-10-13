import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import diff3Merge from "diff3";
import {
  MAX_DIFF_INPUT_BYTES,
  MAX_DIFF_LINES,
  assertRefSupportedModes,
  assertSupportedModes,
  binary,
  commitMessage,
  conflictEntries,
  localIdentity,
  resolveCommitRef,
  sameSnapshot,
  snapshotWithBytes,
  text,
  treeSnapshotMap,
  worktreeSnapshot,
  withIndex,
} from "./git.ts";
import type { MutationResult, Repository, Snapshot } from "./git.ts";
import { fetchCommand, publishRefs, validRef } from "./transfer.ts";
import type { Credentials } from "./remote.ts";
import { refspecDestination, upstream } from "./remote.ts";
import { rebaseCommand, rebaseForkPoint, rebaseInProgress } from "./rebase.ts";

type Conflict = { base?: Snapshot; ours?: Snapshot; theirs?: Snapshot };
export type MergeTree = {
  files: Map<string, Snapshot>;
  conflicts: Map<string, Conflict>;
};

export function safePath(file: string): void {
  if (
    !file ||
    file.includes("\0") ||
    (path.sep === "\\" && file.includes("\\")) ||
    file
      .split("/")
      .some(
        (part) =>
          !part ||
          part === "." ||
          part === ".." ||
          part.toLowerCase() === ".git",
      )
  )
    throw new Error(`unsafe integration path: ${file}`);
}

function checkRenames(
  base: Map<string, Snapshot>,
  side: Map<string, Snapshot>,
): void {
  const structural = (file: string, other: Map<string, Snapshot>) =>
    [...other.keys()].some(
      (path) => path.startsWith(`${file}/`) || file.startsWith(`${path}/`),
    );
  const deleted = [...base.keys()].some(
    (file) => !side.has(file) && !structural(file, side),
  );
  const added = [...side.keys()].some(
    (file) => !base.has(file) && !structural(file, base),
  );
  if (deleted && added)
    throw new Error(
      "rename-sensitive integration (a side both adds and deletes paths) is not supported; no index or worktree changes were made",
    );
}

/** Merge immutable trees before touching the index, worktree or branch. */
export async function mergeTrees(
  repo: Repository,
  base: Map<string, Snapshot>,
  ours: Map<string, Snapshot>,
  theirs: Map<string, Snapshot>,
  theirName: string,
): Promise<MergeTree> {
  checkRenames(base, ours);
  checkRenames(base, theirs);
  const files = new Map<string, Snapshot>();
  const conflicts = new Map<string, Conflict>();
  const paths = new Set([...base.keys(), ...ours.keys(), ...theirs.keys()]);
  for (const file of paths) {
    safePath(file);
    for (const snapshot of [base.get(file), ours.get(file), theirs.get(file)]) {
      if (snapshot?.mode === 0o160000)
        throw new Error(`submodule integration is not supported: ${file}`);
    }
    const b = base.get(file),
      o = ours.get(file),
      t = theirs.get(file);
    let result: Snapshot | undefined;
    if (sameSnapshot(o ?? {}, t ?? {})) result = o;
    else if (sameSnapshot(b ?? {}, o ?? {})) result = t;
    else if (sameSnapshot(b ?? {}, t ?? {})) result = o;
    else {
      const conflict: Conflict = { base: b, ours: o, theirs: t };
      if (
        o &&
        t &&
        [o.mode, t.mode].every((mode) => mode === 0o100644 || mode === 0o100755)
      ) {
        const [baseBytes, ourBytes, theirBytes] = await Promise.all([
          snapshotWithBytes(repo, b),
          snapshotWithBytes(repo, o),
          snapshotWithBytes(repo, t),
        ]);
        if (
          [baseBytes.bytes, ourBytes.bytes, theirBytes.bytes].reduce(
            (size, bytes) => size + (bytes?.byteLength ?? 0),
            0,
          ) > MAX_DIFF_INPUT_BYTES
        )
          throw new Error(
            `merge input exceeds the ${MAX_DIFF_INPUT_BYTES}-byte computation limit: ${file}`,
          );
        const mode =
          o.mode === t.mode ? o.mode : o.mode === b?.mode ? t.mode : o.mode;
        if (![baseBytes.bytes, ourBytes.bytes, theirBytes.bytes].some(binary)) {
          if (
            [baseBytes.bytes, ourBytes.bytes, theirBytes.bytes].reduce(
              (count, bytes) => count + text(bytes).split("\n").length,
              0,
            ) > MAX_DIFF_LINES
          )
            throw new Error(
              `merge input exceeds the ${MAX_DIFF_LINES}-line computation limit: ${file}`,
            );
          const lines = (bytes: Uint8Array | undefined) =>
            text(bytes).match(/[^\n]*\n|[^\n]+$/g) ?? [];
          const merged = diff3Merge(
            lines(ourBytes.bytes),
            lines(baseBytes.bytes),
            lines(theirBytes.bytes),
          );
          const terminated = (lines: string[]) => {
            const value = lines.join("");
            return value && !value.endsWith("\n") ? `${value}\n` : value;
          };
          const bytes = new TextEncoder().encode(
            merged
              .map((block) =>
                block.ok
                  ? block.ok.join("")
                  : `<<<<<<< HEAD\n${terminated(block.conflict.a)}=======\n${terminated(block.conflict.b)}>>>>>>> ${theirName}\n`,
              )
              .join(""),
          );
          result = {
            mode,
            oid: await git.writeBlob({ fs, ...repo, blob: bytes }),
            bytes,
          };
          if (merged.some((block) => "conflict" in block))
            conflicts.set(file, conflict);
        } else {
          result = o;
          conflicts.set(file, conflict);
        }
      } else {
        result = o ?? t;
        conflicts.set(file, conflict);
      }
    }
    if (result) files.set(file, result);
  }
  const unique = (file: string) => {
    let name = file,
      counter = 0;
    while (files.has(name) || paths.has(name)) name = `${file}_${counter++}`;
    safePath(name);
    return name;
  };
  // Git keeps a directory in place and relocates the conflicting file side.
  for (const [file, snapshot] of [...files]) {
    if (![...files.keys()].some((other) => other.startsWith(`${file}/`)))
      continue;
    const ourFile = ours.get(file),
      theirFile = theirs.get(file);
    const relocated = unique(
      `${file}~${ourFile ? "HEAD" : theirName.replace(/\//g, "_")}`,
    );
    files.delete(file);
    files.set(relocated, snapshot);
    conflicts.delete(file);
    conflicts.set(relocated, {
      base: base.get(file),
      ours: ourFile,
      theirs: theirFile,
    });
  }
  // A symlink and a regular file cannot occupy the same worktree pathname.
  for (const [file, conflict] of [...conflicts]) {
    if (
      !conflict.ours ||
      !conflict.theirs ||
      (conflict.ours.mode === 0o120000) === (conflict.theirs.mode === 0o120000)
    )
      continue;
    const ourLink = conflict.ours.mode === 0o120000;
    const relocated = unique(
      `${file}~${ourLink ? theirName.replace(/\//g, "_") : "HEAD"}`,
    );
    files.set(file, ourLink ? conflict.ours : conflict.theirs);
    files.set(relocated, ourLink ? conflict.theirs : conflict.ours);
    conflicts.set(file, {
      base: conflict.base?.mode === 0o120000 ? conflict.base : undefined,
      ours: ourLink ? conflict.ours : undefined,
      theirs: ourLink ? undefined : conflict.theirs,
    });
    conflicts.set(relocated, {
      base: conflict.base?.mode === 0o120000 ? undefined : conflict.base,
      ours: ourLink ? undefined : conflict.ours,
      theirs: ourLink ? conflict.theirs : undefined,
    });
  }
  return { files, conflicts };
}

type SavedFile = { mode?: number; bytes?: string };
export type Change = {
  path: string;
  before: SavedFile;
  after: SavedFile;
  write: boolean;
};
type MergeState = {
  phase: "install" | "conflict" | "commit" | "abort" | "cleanup";
  ref: string;
  ours?: string;
  theirs: string;
  message: string;
  originalIndex: string;
  nextIndex: string;
  changes: Change[];
  conflicts: string[];
  result?: string;
};
const journal = "golem-merge-state";
async function optionalBytes(file: string): Promise<Buffer | undefined> {
  try {
    return await fs.promises.readFile(file);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}
export async function atomicFile(
  repo: Repository,
  file: string,
  bytes: string | Uint8Array,
): Promise<void> {
  const target = path.join(repo.gitdir, file);
  await fs.promises.writeFile(`${target}.lock`, bytes);
  await fs.promises.rename(`${target}.lock`, target);
}
async function saveState(repo: Repository, state: MergeState): Promise<void> {
  await atomicFile(repo, journal, JSON.stringify(state));
}
async function stateFor(repo: Repository): Promise<MergeState | undefined> {
  const bytes = await optionalBytes(path.join(repo.gitdir, journal));
  if (!bytes) return undefined;
  const state: MergeState = JSON.parse(bytes.toString());
  if (
    !Array.isArray(state.changes) ||
    !Array.isArray(state.conflicts) ||
    typeof state.message !== "string" ||
    typeof state.originalIndex !== "string" ||
    typeof state.nextIndex !== "string" ||
    !["install", "conflict", "commit", "abort", "cleanup"].includes(
      state.phase,
    ) ||
    !/^(HEAD|refs\/heads\/.+)$/.test(state.ref) ||
    state.ref.includes("..") ||
    state.ref.includes("\\")
  )
    throw new Error("invalid merge recovery state");
  if (state.ref !== "HEAD") validRef(state.ref);
  for (const oid of [state.ours, state.theirs, state.result].filter(Boolean))
    if (!/^[0-9a-f]{40}$/.test(oid!))
      throw new Error("invalid merge recovery object ID");
  if (
    !state.theirs ||
    ((state.phase === "commit" || (!state.ours && state.phase === "install")) &&
      !state.result) ||
    ((state.phase === "abort" || state.phase === "conflict") && !state.ours)
  )
    throw new Error("invalid merge recovery phase");
  for (const change of state.changes) {
    safePath(change.path);
    if (typeof change.write !== "boolean")
      throw new Error("invalid merge worktree publication");
    for (const file of [change.before, change.after]) {
      if (
        !file ||
        (file.bytes !== undefined &&
          (typeof file.bytes !== "string" ||
            ![0o100644, 0o100755, 0o120000].includes(file.mode!)))
      )
        throw new Error("invalid merge recovery file");
    }
  }
  for (const file of state.conflicts) safePath(file);
  return state;
}
async function saved(
  repo: Repository,
  snapshot?: Snapshot,
): Promise<SavedFile> {
  if (!snapshot?.oid) return {};
  const materialized = snapshot.bytes
    ? snapshot
    : await snapshotWithBytes(repo, snapshot);
  return {
    mode: materialized.mode,
    bytes: Buffer.from(materialized.bytes!).toString("base64"),
  };
}
export async function installFiles(
  repo: Repository,
  changes: Change[],
  inverse: boolean,
  hardReset = false,
): Promise<void> {
  changes = changes.filter((change) => inverse || change.write);
  for (const change of [...changes].sort(
    (a, b) => b.path.length - a.path.length,
  )) {
    let parentPath = "",
      blocked = false;
    for (const segment of change.path.split("/").slice(0, -1)) {
      parentPath = parentPath ? `${parentPath}/${segment}` : segment;
      const parent = await fs.promises
        .lstat(path.join(repo.dir, parentPath))
        .catch((error) => {
          if (error.code === "ENOENT") return undefined;
          throw error;
        });
      if (parent && (!parent.isDirectory() || parent.isSymbolicLink())) {
        if (!changes.some((other) => other.path === parentPath))
          throw new Error(`unsafe integration ancestor: ${change.path}`);
        blocked = true;
        break;
      }
    }
    if (blocked) continue;
    const absolute = path.join(repo.dir, change.path);
    const metadata = await fs.promises.lstat(absolute).catch((error) => {
      if (error.code === "ENOENT" || error.code === "ENOTDIR") return undefined;
      throw error;
    });
    if (metadata?.isDirectory()) {
      if (hardReset)
        await fs.promises.rm(absolute, { recursive: true, force: true });
      else await fs.promises.rmdir(absolute);
    } else if (metadata) await fs.promises.unlink(absolute);
    let parent = path.dirname(absolute);
    while (parent !== repo.dir) {
      try {
        await fs.promises.rmdir(parent);
      } catch (error) {
        if (
          ["ENOTEMPTY", "EEXIST", "ENOTDIR"].includes(
            (error as NodeJS.ErrnoException).code!,
          )
        )
          break;
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
      parent = path.dirname(parent);
    }
  }
  for (const change of changes) {
    const snapshot = inverse ? change.before : change.after;
    if (snapshot.bytes === undefined) continue;
    const absolute = path.join(repo.dir, change.path);
    let parent = repo.dir;
    for (const segment of change.path.split("/").slice(0, -1)) {
      parent = path.join(parent, segment);
      const metadata = await fs.promises.lstat(parent).catch((error) => {
        if (error.code === "ENOENT") return undefined;
        throw error;
      });
      if (metadata && (!metadata.isDirectory() || metadata.isSymbolicLink()))
        throw new Error(`unsafe integration ancestor: ${change.path}`);
      if (!metadata) await fs.promises.mkdir(parent);
    }
    const bytes = Buffer.from(snapshot.bytes, "base64");
    if (snapshot.mode === 0o120000)
      await fs.promises.symlink(bytes.toString(), absolute);
    else {
      await fs.promises.writeFile(absolute, bytes);
      await fs.promises.chmod(
        absolute,
        snapshot.mode === 0o100755 ? 0o755 : 0o644,
      );
    }
  }
}

async function verifyHead(repo: Repository, state: MergeState): Promise<void> {
  const branch =
    (await git.currentBranch({ fs, ...repo, fullname: true })) ?? "HEAD";
  let head: string | undefined;
  try {
    head = await git.resolveRef({ fs, ...repo, ref: "HEAD" });
  } catch (error) {
    if ((error as { code?: string }).code !== "NotFoundError") throw error;
  }
  if (
    branch !== state.ref ||
    (head !== state.ours && (!state.result || head !== state.result))
  )
    throw new Error(
      "HEAD changed since integration began; refusing to overwrite the changed branch",
    );
}

export async function recoverMerge(repo: Repository): Promise<void> {
  const state = await stateFor(repo);
  if (!state || state.phase === "conflict") return;
  if (state.phase === "cleanup") {
    await clearMergeFiles(repo);
    return;
  }
  await verifyHead(repo, state);
  if (state.phase === "install" || state.phase === "abort") {
    await installFiles(repo, state.changes, state.phase === "abort");
    const index =
      state.phase === "abort" ? state.originalIndex : state.nextIndex;
    if (index) await atomicFile(repo, "index", Buffer.from(index, "base64"));
    else await fs.promises.rm(path.join(repo.gitdir, "index"), { force: true });
  }
  if (state.ours) await atomicFile(repo, "ORIG_HEAD", `${state.ours}\n`);
  if (state.phase === "install" && !state.result) {
    await atomicFile(repo, "MERGE_HEAD", `${state.theirs}\n`);
    await atomicFile(repo, "MERGE_MSG", state.message);
    state.phase = "conflict";
    await saveState(repo, state);
    return;
  }
  if (state.phase === "install") {
    state.phase = "commit";
    await saveState(repo, state);
  }
  await publishRefs(repo, {
    writes: [
      [state.ref, `${state.phase === "abort" ? state.ours! : state.result!}\n`],
    ],
    deletions: [],
  });
  state.phase = "cleanup";
  await saveState(repo, state);
  await clearMergeFiles(repo);
}

async function clearMergeFiles(repo: Repository): Promise<void> {
  for (const file of ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", journal])
    await fs.promises.rm(path.join(repo.gitdir, file), { force: true });
}

export async function discardMerge(repo: Repository): Promise<void> {
  const state = await stateFor(repo);
  if (!state) return;
  state.phase = "cleanup";
  await saveState(repo, state);
  await clearMergeFiles(repo);
}

export async function treeOid(
  repo: Repository,
  files: Map<string, Snapshot>,
  prefix = "",
): Promise<string> {
  const entries = new Map<
    string,
    { mode: string; path: string; oid: string; type: "tree" | "blob" }
  >();
  for (const [file, snapshot] of files) {
    if (!file.startsWith(prefix)) continue;
    const name = file.slice(prefix.length).split("/")[0]!;
    if (entries.has(name)) continue;
    if (file.slice(prefix.length).includes("/"))
      entries.set(name, {
        mode: "040000",
        path: name,
        oid: await treeOid(repo, files, `${prefix}${name}/`),
        type: "tree",
      });
    else
      entries.set(name, {
        mode: snapshot.mode!.toString(8),
        path: name,
        oid: snapshot.oid!,
        type: "blob",
      });
  }
  return git.writeTree({ fs, ...repo, tree: [...entries.values()] });
}

export async function prepareIndexAndWorktree(
  repo: Repository,
  head: Map<string, Snapshot>,
  merged: MergeTree,
  fastForward: boolean,
  hardReset = false,
) {
  const original = await optionalBytes(path.join(repo.gitdir, "index"));
  return withIndex(repo, original, async (index) => {
    if (index.unmergedPaths.length && !hardReset)
      throw new Error("resolve the existing unmerged index first");
    const indexed = new Map<string, Snapshot>(
      index.entries.map(
        (entry: { path: string; oid: string; mode: number }) => [
          entry.path,
          { oid: entry.oid, mode: entry.mode },
        ],
      ),
    );
    const changes: Change[] = [];
    const resetAncestors = new Set<string>();
    for (const file of new Set([
      ...head.keys(),
      ...indexed.keys(),
      ...merged.files.keys(),
      ...merged.conflicts.keys(),
    ])) {
      safePath(file);
      const h = head.get(file) ?? {},
        i = indexed.get(file) ?? {},
        target = merged.files.get(file) ?? {};
      if (
        !hardReset &&
        fastForward &&
        sameSnapshot(h, target) &&
        !merged.conflicts.has(file)
      )
        continue;
      const worktree = await worktreeSnapshot(repo, file);
      if (
        !hardReset &&
        !i.oid &&
        worktree.oid &&
        target.oid &&
        !(await git.isIgnored({ fs, ...repo, filepath: file }))
      )
        throw new Error(`untracked file would be overwritten: ${file}`);
      if (!hardReset && !fastForward && !sameSnapshot(h, i))
        throw new Error(
          `local index changes would be overwritten by merge: ${file}`,
        );
      if (!hardReset && !sameSnapshot(h, i) && !sameSnapshot(i, target))
        throw new Error(`local index changes would be overwritten: ${file}`);
      if (
        !hardReset &&
        !sameSnapshot(i, target) &&
        i.oid &&
        !sameSnapshot(i, worktree)
      )
        throw new Error(`local worktree changes would be overwritten: ${file}`);
      if (!hardReset && sameSnapshot(i, target) && !merged.conflicts.has(file))
        continue;
      let ancestor = "";
      for (const segment of file.split("/").slice(0, -1)) {
        ancestor = ancestor ? `${ancestor}/${segment}` : segment;
        const metadata = await fs.promises
          .lstat(path.join(repo.dir, ancestor))
          .catch((error) => {
            if (error.code === "ENOENT") return undefined;
            throw error;
          });
        if (!metadata) break;
        if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
          if (hardReset && !merged.files.has(ancestor)) {
            resetAncestors.add(ancestor);
            break;
          }
          if (head.has(ancestor) && !merged.files.has(ancestor)) break;
          throw new Error(
            `untracked or symbolic-link ancestor would be overwritten: ${file}`,
          );
        }
      }
      const metadata = await fs.promises
        .lstat(path.join(repo.dir, file))
        .catch((error) => {
          if (["ENOENT", "ENOTDIR"].includes(error.code)) return undefined;
          throw error;
        });
      if (!hardReset && metadata?.isDirectory() && target.oid) {
        const visit = async (dir: string): Promise<void> => {
          for (const name of await fs.promises.readdir(
            path.join(repo.dir, dir),
          )) {
            const entry = `${dir}/${name}`;
            const stat = await fs.promises.lstat(path.join(repo.dir, entry));
            if (stat.isDirectory() && !stat.isSymbolicLink())
              await visit(entry);
            else if (!indexed.has(entry))
              throw new Error(`untracked file would be overwritten: ${entry}`);
          }
        };
        await visit(file);
      }
      changes.push({
        path: file,
        before: await saved(repo, h),
        after: await saved(repo, target),
        write: hardReset || !sameSnapshot(i, target),
      });
      index.delete({ filepath: file });
      const conflict = merged.conflicts.get(file);
      if (conflict) {
        for (const [stage, snapshot] of [
          [1, conflict.base],
          [2, conflict.ours],
          [3, conflict.theirs],
        ] as const) {
          if (snapshot?.oid)
            index.insert({
              filepath: file,
              oid: snapshot.oid,
              stage,
              stats: {
                mode: snapshot.mode,
                ctimeMs: 0,
                mtimeMs: 0,
                dev: 0,
                ino: 0,
                uid: 0,
                gid: 0,
                size: 0,
              },
            });
        }
      } else if (target.oid)
        index.insert({
          filepath: file,
          oid: target.oid,
          stats: {
            mode: target.mode,
            ctimeMs: 0,
            mtimeMs: 0,
            dev: 0,
            ino: 0,
            uid: 0,
            gid: 0,
            size: 0,
          },
        });
    }
    for (const ancestor of resetAncestors) {
      if (!changes.some((change) => change.path === ancestor))
        changes.push({ path: ancestor, before: {}, after: {}, write: true });
    }
    return {
      originalIndex: original?.toString("base64") ?? "",
      nextIndex: (await index.toObject()).toString("base64"),
      changes,
    };
  });
}

export type MergeOptions = {
  ffOnly?: boolean;
  noFf?: boolean;
  ff?: boolean;
  message?: string;
};
export async function mergeCommand(
  repo: Repository,
  theirRef: string | undefined,
  options: MergeOptions = {},
): Promise<MutationResult> {
  await assertSupportedModes(repo);
  if (
    (await rebaseInProgress(repo)) ||
    (await stateFor(repo)) ||
    (await optionalBytes(path.join(repo.gitdir, "MERGE_HEAD")))
  )
    throw new Error(
      "a merge is already in progress; use --continue or --abort",
    );
  if ([options.ffOnly, options.noFf, options.ff].filter(Boolean).length > 1)
    throw new Error("fast-forward options are mutually exclusive");
  if (!theirRef) {
    const branch = await git.currentBranch({ fs, ...repo });
    if (!branch) throw new Error("specify a revision when HEAD is detached");
    const merge = await git.getConfigAll({
      fs,
      ...repo,
      path: `branch.${branch}.merge`,
    });
    if (merge.length > 1) throw new Error("octopus merges are not supported");
    theirRef = (await upstream(repo, branch))?.ref;
    if (!theirRef)
      throw new Error("no upstream configured; specify a merge revision");
  }
  if (theirRef === "FETCH_HEAD") {
    const candidates = (
      await fs.promises.readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8")
    )
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split("\t"))
      .filter(([, merge]) => merge === "");
    if (candidates.length !== 1)
      throw new Error(
        "FETCH_HEAD must contain exactly one merge candidate; octopus merges are not supported",
      );
    theirRef = candidates[0]![0]!;
  }
  const theirName = theirRef;
  let ours: string | undefined;
  try {
    ours = await resolveCommitRef(repo, "HEAD");
  } catch (error) {
    if ((error as { code?: string }).code !== "NotFoundError") throw error;
    if (!(await git.currentBranch({ fs, ...repo }))) throw error;
  }
  const theirs = await resolveCommitRef(repo, theirRef);
  await assertRefSupportedModes(repo, theirs);
  const bases = ours
    ? await git.findMergeBase({ fs, ...repo, oids: [ours, theirs] })
    : [];
  if (bases.includes(theirs))
    return { summary: "Already up to date", paths: [] };
  if (ours && bases.length !== 1)
    throw new Error(
      bases.length
        ? "multiple-base merge is not supported; no index or worktree changes were made"
        : "refusing to merge unrelated histories",
    );
  const canFf = !ours || bases[0] === ours;
  if (options.ffOnly && !canFf)
    throw new Error("cannot fast-forward divergent branches");
  const configured = await git.getConfig({ fs, ...repo, path: "merge.ff" });
  const explicit = options.ff || options.ffOnly || options.noFf;
  const noFf =
    options.noFf ||
    (!explicit && (configured === false || configured === "false"));
  if (!ours && noFf)
    throw new Error(
      "non-fast-forward merge into an empty branch is not supported",
    );
  if (configured === "only" && !canFf && !explicit)
    throw new Error("merge.ff=only requires a fast-forward");
  const fastForward = canFf && !noFf;
  const head = ours
    ? await treeSnapshotMap(repo, ours)
    : new Map<string, Snapshot>();
  const merged: MergeTree = fastForward
    ? { files: await treeSnapshotMap(repo, theirs), conflicts: new Map() }
    : await mergeTrees(
        repo,
        await treeSnapshotMap(repo, bases[0]!),
        head,
        await treeSnapshotMap(repo, theirs),
        theirName,
      );
  const prepared = await prepareIndexAndWorktree(
    repo,
    head,
    merged,
    fastForward,
  );
  const identity = fastForward ? undefined : await localIdentity(repo);
  const message =
    options.message !== undefined
      ? commitMessage([options.message])
      : `Merge branch '${theirName}'\n`;
  const result = fastForward
    ? theirs
    : merged.conflicts.size
      ? undefined
      : await git.writeCommit({
          fs,
          ...repo,
          commit: {
            tree: await treeOid(repo, merged.files),
            parent: [ours!, theirs],
            author: {
              ...identity!,
              timestamp: Math.floor(Date.now() / 1000),
              timezoneOffset: new Date().getTimezoneOffset(),
            },
            committer: {
              ...identity!,
              timestamp: Math.floor(Date.now() / 1000),
              timezoneOffset: new Date().getTimezoneOffset(),
            },
            message,
          },
        });
  const ref =
    (await git.currentBranch({ fs, ...repo, fullname: true })) ?? "HEAD";
  const state: MergeState = {
    phase: "install",
    ref,
    ours,
    theirs,
    message,
    ...prepared,
    conflicts: [...merged.conflicts.keys()],
    result,
  };
  await saveState(repo, state);
  await recoverMerge(repo);
  if (state.conflicts.length)
    throw new Error(
      `merge conflicts: ${state.conflicts.join(", ")}; resolve and add paths, then commit or merge --continue`,
    );
  return {
    summary: fastForward ? "Fast-forward" : "Merge made",
    paths: state.changes.map((change) => change.path),
  };
}

export async function continueMerge(
  repo: Repository,
  messages?: readonly string[],
  author?: { name: string; email: string },
): Promise<string> {
  const state = await stateFor(repo);
  if (!state || state.phase !== "conflict")
    throw new Error("no merge is in progress");
  await verifyHead(repo, state);
  const identity = await localIdentity(repo);
  const result = await git.commit({
    fs,
    ...repo,
    message: messages?.length ? messages.join("\n\n") : state.message,
    parent: [state.ours!, state.theirs],
    author: author ?? identity,
    committer: identity,
    noUpdateBranch: true,
  });
  state.result = result;
  state.phase = "commit";
  await saveState(repo, state);
  await recoverMerge(repo);
  return result;
}

export async function abortMerge(repo: Repository): Promise<MutationResult> {
  const state = await stateFor(repo);
  if (!state || state.phase !== "conflict")
    throw new Error("no merge is in progress");
  await verifyHead(repo, state);
  const indexed = await withIndex(
    repo,
    await optionalBytes(path.join(repo.gitdir, "index")),
    async (index) => ({
      unmerged: new Set(index.unmergedPaths),
      files: new Map<string, Snapshot>(
        index.entries.map((entry) => [
          entry.path,
          { oid: entry.oid, mode: entry.mode },
        ]),
      ),
    }),
  );
  const original = await withIndex(
    repo,
    state.originalIndex
      ? Buffer.from(state.originalIndex, "base64")
      : undefined,
    async (index) =>
      new Map<string, Snapshot>(
        index.entries.map((entry) => [
          entry.path,
          { oid: entry.oid, mode: entry.mode },
        ]),
      ),
  );
  for (const change of state.changes) {
    if (indexed.unmerged.has(change.path)) continue;
    const current = indexed.files.get(change.path) ?? {};
    const worktree = await worktreeSnapshot(repo, change.path);
    if (sameSnapshot(current, original.get(change.path) ?? {})) {
      change.before = await saved(repo, worktree);
      continue;
    }
    if (!sameSnapshot(current, worktree))
      throw new Error(
        `local worktree changes prevent merge abort: ${change.path}`,
      );
  }
  state.phase = "abort";
  await saveState(repo, state);
  await recoverMerge(repo);
  return {
    summary: "Merge aborted",
    paths: state.changes.map((change) => change.path),
  };
}

export async function mergeInProgress(repo: Repository): Promise<boolean> {
  return (await stateFor(repo))?.phase === "conflict";
}

export type PullOptions = MergeOptions & { rebase?: boolean };
export async function pullCommand(
  repo: Repository,
  remote: string | undefined,
  refspecs: string[],
  options: PullOptions,
  send: typeof fetch,
  credentials?: Credentials,
): Promise<MutationResult> {
  if (
    (await mergeInProgress(repo)) ||
    (await rebaseInProgress(repo)) ||
    (await conflictEntries(repo)).size
  )
    throw new Error("resolve the current integration before pulling");
  const branch = await git.currentBranch({ fs, ...repo });
  let forkPoint: string | undefined;
  if (branch) {
    try {
      const tracking = await upstream(repo, branch);
      if (tracking)
        forkPoint = await rebaseForkPoint(
          repo,
          tracking.ref,
          await resolveCommitRef(repo, "HEAD"),
        );
    } catch (error) {
      if (
        (error as { code?: string }).code !== "NotFoundError" &&
        !/upstream is (not mapped|excluded)/.test((error as Error).message)
      )
        throw error;
    }
  }
  const configuredRemote = branch
    ? await git.getConfig({ fs, ...repo, path: `branch.${branch}.remote` })
    : undefined;
  const mergeSources = branch
    ? await git.getConfigAll({ fs, ...repo, path: `branch.${branch}.merge` })
    : [];
  if (
    !remote &&
    !refspecs.length &&
    (!configuredRemote || !mergeSources.length)
  )
    throw new Error("no upstream configured; specify a remote and branch");
  remote ??= configuredRemote ?? "origin";
  if (refspecs.length || remote !== configuredRemote) {
    forkPoint = undefined;
    if (refspecs.length === 1) {
      const source = refspecs[0]!.replace(/^\+/, "").split(":")[0]!;
      const full = source.startsWith("refs/") ? source : `refs/heads/${source}`;
      const specs: string[] = await git.getConfigAll({
        fs,
        ...repo,
        path: `remote.${remote}.fetch`,
      });
      const mapped = specs
        .map((spec) => refspecDestination(spec, full))
        .find(Boolean);
      if (mapped) {
        try {
          forkPoint = await rebaseForkPoint(
            repo,
            mapped,
            await resolveCommitRef(repo, "HEAD"),
          );
        } catch (error) {
          if ((error as { code?: string }).code !== "NotFoundError")
            throw error;
        }
      }
    }
  }
  let candidates: string[];
  if (remote === ".") {
    const sources = refspecs.length
      ? refspecs
      : configuredRemote === "."
        ? mergeSources
        : ["HEAD"];
    if (sources.length > 1) throw new Error("octopus pulls are not supported");
    const ref = sources[0] ?? "HEAD";
    candidates = [await resolveCommitRef(repo, ref)];
    await atomicFile(
      repo,
      "FETCH_HEAD",
      `${candidates[0]}\t\tbranch '${ref.replace(/^refs\/heads\//, "")}' of .\n`,
    );
  } else {
    await fetchCommand(repo, remote, refspecs, {}, send, credentials);
    candidates = (
      await fs.promises.readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8")
    )
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split("\t"))
      .filter(([, merge]) => merge === "")
      .map(([oid]) => oid!);
  }
  if (candidates.length !== 1)
    throw new Error(
      candidates.length
        ? "octopus pulls are not supported"
        : "no fetched merge candidate; specify the remote branch",
    );
  const configuredRebase = branch
    ? await git.getConfig({ fs, ...repo, path: `branch.${branch}.rebase` })
    : undefined;
  const rebase =
    options.rebase ??
    configuredRebase ??
    (await git.getConfig({ fs, ...repo, path: "pull.rebase" }));
  const rebasing =
    rebase !== undefined &&
    ![false, "false", "0", "off", "no"].includes(rebase);
  if (rebasing && ![true, "true", "1", "on", "yes", ""].includes(rebase))
    throw new Error(`unsupported pull rebase strategy: ${rebase}`);
  const ff = await git.getConfig({ fs, ...repo, path: "pull.ff" });
  const explicit = options.ff || options.noFf || options.ffOnly;
  const strategy = explicit
    ? options
    : {
        ...options,
        ffOnly: ff === "only",
        noFf: ff === false || ff === "false",
        ff: ff === true || ff === "true",
      };
  if (rebasing) {
    const ours = await resolveCommitRef(repo, "HEAD");
    const bases = await git.findMergeBase({
      fs,
      ...repo,
      oids: [ours, candidates[0]!],
    });
    if (
      strategy.ffOnly &&
      !bases.includes(ours) &&
      !bases.includes(candidates[0]!)
    )
      throw new Error("cannot fast-forward divergent branches");
    if (bases.includes(ours))
      return mergeCommand(repo, candidates[0]!, { ffOnly: true });
    return rebaseCommand(repo, candidates[0]!, undefined, forkPoint);
  }
  if (!explicit && ff === undefined && rebase === undefined) {
    try {
      const ours = await resolveCommitRef(repo, "HEAD");
      const bases = await git.findMergeBase({
        fs,
        ...repo,
        oids: [ours, candidates[0]!],
      });
      if (!bases.includes(ours) && !bases.includes(candidates[0]!))
        throw new Error(
          "divergent branches require an explicit pull merge/rebase strategy",
        );
    } catch (error) {
      if ((error as { code?: string }).code !== "NotFoundError") throw error;
    }
  }
  return mergeCommand(repo, candidates[0]!, strategy);
}
