import fs from "node:fs";
import path from "node:path";
import * as git from "../golem-temp/isomorphic-git/index.js";
import { GitIndex } from "../golem-temp/isomorphic-git/models/index.js";
import {
  assertRefSupportedModes,
  assertSupportedModes,
  checkoutCommand,
  conflictEntries,
  localIdentity,
  resolveCommitRef,
  sameSnapshot,
  statusEntries,
  treeSnapshotMap,
} from "./git.ts";
import type { MutationResult, Repository, Snapshot } from "./git.ts";
import {
  atomicFile,
  installFiles,
  mergeInProgress,
  mergeTrees,
  prepareIndexAndWorktree,
  safePath,
  treeOid,
} from "./integration.ts";
import type { Change } from "./integration.ts";
import { upstream } from "./remote.ts";
import { publishRefs, validRef } from "./transfer.ts";
import { assertRebaseAttributes, patchIds } from "./patch-id.ts";

type Action = {
  before: string;
  after: string;
  index?: string;
  changes: Change[];
  hard: boolean;
  next: "ready" | "conflict" | "finish";
  advance: boolean;
};
type State = {
  phase: "install" | "ready" | "conflict" | "finish" | "cleanup";
  original: string;
  ref: string;
  onto: string;
  todo: string[];
  position: number;
  current: string;
  resolved?: string;
  action?: Action;
};
const directory = "rebase-merge";
const journal = "golem-rebase-state";
function recoveryBytes(value: string): Buffer {
  if (typeof value !== "string")
    throw new Error("invalid rebase recovery bytes");
  const bytes = Buffer.from(value, "base64");
  if (bytes.toString("base64") !== value)
    throw new Error("invalid rebase recovery bytes");
  return bytes;
}
async function stateFor(repo: Repository): Promise<State | undefined> {
  let bytes: string;
  try {
    bytes = await fs.promises.readFile(path.join(repo.gitdir, journal), "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
  const state: State = JSON.parse(bytes);
  if (
    !Array.isArray(state.todo) ||
    !Number.isInteger(state.position) ||
    state.position < 0 ||
    state.position > state.todo.length ||
    !["install", "ready", "conflict", "finish", "cleanup"].includes(
      state.phase,
    ) ||
    (state.ref !== "HEAD" && !state.ref?.startsWith("refs/heads/"))
  )
    throw new Error("invalid rebase recovery state");
  if (state.ref !== "HEAD") validRef(state.ref);
  if (
    ![state.original, state.onto, state.current].every(
      (oid) => typeof oid === "string" && /^[0-9a-f]{40}$/.test(oid),
    ) ||
    (state.phase === "conflict" && state.position === state.todo.length)
  )
    throw new Error("invalid rebase recovery state");
  for (const oid of [
    state.original,
    state.onto,
    state.current,
    ...state.todo,
    state.resolved,
  ].filter((oid) => oid !== undefined))
    if (typeof oid !== "string" || !/^[0-9a-f]{40}$/.test(oid))
      throw new Error("invalid rebase recovery object ID");
  if (state.phase !== "install" && state.action !== undefined)
    throw new Error("invalid rebase publication phase");
  if (state.phase === "install") {
    const action = state.action;
    if (
      !action ||
      !Array.isArray(action.changes) ||
      !["ready", "conflict", "finish"].includes(action.next) ||
      typeof action.hard !== "boolean" ||
      typeof action.advance !== "boolean" ||
      action.before !== state.current ||
      state.position + Number(action.advance) > state.todo.length ||
      (action.next === "conflict" &&
        state.position + Number(action.advance) >= state.todo.length) ||
      ![action.before, action.after].every(
        (oid) => typeof oid === "string" && /^[0-9a-f]{40}$/.test(oid),
      )
    )
      throw new Error("invalid rebase publication state");
    if (action.index !== undefined) {
      const bytes = recoveryBytes(action.index);
      const index = await GitIndex.from(bytes);
      if (!(await index.toObject()).equals(bytes))
        throw new Error("invalid rebase recovery index");
      for (const entry of index.entries) safePath(entry.path);
    }
    for (const change of action.changes) {
      safePath(change.path);
      if (typeof change.write !== "boolean")
        throw new Error("invalid rebase file publication");
      for (const snapshot of [change.before, change.after])
        if (
          !snapshot ||
          typeof snapshot !== "object" ||
          Array.isArray(snapshot) ||
          (Object.keys(snapshot).length !== 0 &&
            (typeof snapshot.bytes !== "string" ||
              ![0o100644, 0o100755, 0o120000].includes(snapshot.mode!) ||
              Object.keys(snapshot).some(
                (key) => !["mode", "bytes"].includes(key),
              )))
        )
          throw new Error("invalid rebase recovery file");
        else if (snapshot.bytes !== undefined) recoveryBytes(snapshot.bytes);
    }
  }
  for (const oid of new Set(
    [
      state.original,
      state.onto,
      state.current,
      ...state.todo,
      state.resolved,
      state.action?.after,
    ].filter((oid) => oid !== undefined),
  ))
    await git.readCommit({ fs, ...repo, oid: oid! });
  return state;
}
async function save(repo: Repository, state: State): Promise<void> {
  await atomicFile(repo, journal, JSON.stringify(state));
}
async function verify(
  repo: Repository,
  state: State,
  allowFinished = false,
): Promise<void> {
  const head = await resolveCommitRef(repo, "HEAD");
  const ref =
    (await git.currentBranch({ fs, ...repo, fullname: true })) ?? "HEAD";
  const allowed = state.action
    ? [state.action.before, state.action.after]
    : [state.current];
  if (
    !allowed.includes(head) ||
    (ref !== "HEAD" &&
      !(
        ref === state.ref &&
        (head === state.original || (allowFinished && head === state.current))
      ))
  )
    throw new Error("HEAD changed during rebase; refusing to overwrite it");
  if (state.ref !== "HEAD") {
    const branch = await resolveCommitRef(repo, state.ref);
    if (
      branch !== state.original &&
      !(allowFinished && branch === state.current)
    )
      throw new Error(
        "the original branch changed during rebase; refusing to overwrite it",
      );
  }
}
async function metadata(repo: Repository, state: State): Promise<void> {
  await atomicFile(repo, "ORIG_HEAD", `${state.original}\n`);
  for (const [name, value] of Object.entries({
    "head-name": state.ref,
    "orig-head": state.original,
    onto: state.onto,
    msgnum: String(state.position + 1),
    end: String(state.todo.length),
    "git-rebase-todo": state.todo
      .slice(state.position)
      .map((oid) => `pick ${oid}`)
      .join("\n"),
    done: state.todo
      .slice(0, state.position)
      .map((oid) => `pick ${oid}`)
      .join("\n"),
  }))
    await atomicFile(repo, `${directory}/${name}`, `${value}\n`);
  if (state.phase === "conflict")
    await atomicFile(repo, "REBASE_HEAD", `${state.todo[state.position]}\n`);
  else
    await fs.promises.rm(path.join(repo.gitdir, "REBASE_HEAD"), {
      force: true,
    });
}
export async function recoverRebase(repo: Repository): Promise<void> {
  const state = await stateFor(repo);
  if (!state) return;
  if (state.phase === "cleanup") {
    await fs.promises.rm(path.join(repo.gitdir, "REBASE_HEAD"), {
      force: true,
    });
    await fs.promises.rm(path.join(repo.gitdir, directory), {
      recursive: true,
      force: true,
    });
    await fs.promises.rm(path.join(repo.gitdir, journal), { force: true });
    return;
  }
  await verify(repo, state, state.phase === "finish");
  await fs.promises.mkdir(path.join(repo.gitdir, directory), {
    recursive: true,
  });
  const metadataDirectory = await fs.promises.lstat(
    path.join(repo.gitdir, directory),
  );
  if (!metadataDirectory.isDirectory() || metadataDirectory.isSymbolicLink())
    throw new Error("unsafe rebase metadata directory");
  if (state.phase === "install") {
    const action = state.action!;
    await installFiles(repo, action.changes, false, action.hard);
    if (action.index !== undefined)
      await atomicFile(repo, "index", Buffer.from(action.index, "base64"));
    await atomicFile(repo, "HEAD", `${action.after}\n`);
    state.current = action.after;
    if (action.advance) state.position++;
    state.phase = action.next;
    delete state.action;
    await save(repo, state);
  }
  await metadata(repo, state);
  if (state.phase === "finish") {
    if (state.ref !== "HEAD") {
      await publishRefs(repo, {
        writes: [[state.ref, `${state.current}\n`]],
        deletions: [],
      });
      await atomicFile(repo, "HEAD", `ref: ${state.ref}\n`);
    }
    state.phase = "cleanup";
    await save(repo, state);
    await recoverRebase(repo);
  }
}
async function install(
  repo: Repository,
  state: State,
  action: Action,
): Promise<void> {
  state.phase = "install";
  state.action = action;
  await save(repo, state);
  await recoverRebase(repo);
  const recovered = await stateFor(repo);
  if (recovered) Object.assign(state, recovered);
  delete state.action;
}
async function replayCommit(
  repo: Repository,
  source: string,
  parent: string,
  tree: string,
): Promise<string> {
  const { commit } = await git.readCommit({ fs, ...repo, oid: source });
  const identity = await localIdentity(repo);
  return git.writeCommit({
    fs,
    ...repo,
    commit: {
      tree,
      parent: [parent],
      message: commit.message,
      author: commit.author,
      committer: {
        ...identity,
        timestamp: Math.floor(Date.now() / 1000),
        timezoneOffset: new Date().getTimezoneOffset(),
      },
    },
  });
}
async function replay(repo: Repository, state: State): Promise<MutationResult> {
  while (state.position < state.todo.length) {
    await verify(repo, state);
    const source = state.todo[state.position]!;
    const { commit } = await git.readCommit({ fs, ...repo, oid: source });
    const base = commit.parent[0]
      ? await treeSnapshotMap(repo, commit.parent[0])
      : new Map<string, Snapshot>();
    const head = await treeSnapshotMap(repo, state.current);
    const theirs = await treeSnapshotMap(repo, source);
    const originallyEmpty = [
      ...new Set([...base.keys(), ...theirs.keys()]),
    ].every((file) =>
      sameSnapshot(base.get(file) ?? {}, theirs.get(file) ?? {}),
    );
    const merged = await mergeTrees(
      repo,
      base,
      head,
      theirs,
      source.slice(0, 7),
    );
    const prepared = await prepareIndexAndWorktree(repo, head, merged, false);
    const tree = await treeOid(repo, merged.files);
    const currentTree = (
      await git.readCommit({ fs, ...repo, oid: state.current })
    ).commit.tree;
    const result =
      merged.conflicts.size || (!originallyEmpty && tree === currentTree)
        ? state.current
        : await replayCommit(repo, source, state.current, tree);
    await install(repo, state, {
      before: state.current,
      after: result,
      index: prepared.nextIndex,
      changes: prepared.changes,
      hard: false,
      next: merged.conflicts.size ? "conflict" : "ready",
      advance: !merged.conflicts.size,
    });
    if (merged.conflicts.size)
      throw new Error(
        `rebase conflicts: ${[...merged.conflicts.keys()].join(", ")}; resolve and add paths, then rebase --continue, --skip or --abort`,
      );
  }
  state.phase = "finish";
  await save(repo, state);
  await recoverRebase(repo);
  return { summary: "Rebase completed", paths: [] };
}
export async function rebaseInProgress(repo: Repository): Promise<boolean> {
  return (await stateFor(repo)) !== undefined;
}
export async function rebaseForkPoint(
  repo: Repository,
  ref: string,
  original: string,
): Promise<string | undefined> {
  const full = await git.expandRef({ fs, ...repo, ref });
  let log: string;
  try {
    log = await fs.promises.readFile(
      path.join(repo.gitdir, "logs", full),
      "utf8",
    );
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
  const tips = new Set<string>();
  for (const line of log.split("\n").filter(Boolean)) {
    const match = /^([a-f0-9]{40}) ([a-f0-9]{40}) /.exec(line);
    if (!match) throw new Error("invalid upstream reflog");
    for (const oid of [match[1]!, match[2]!])
      if (oid !== "0".repeat(40)) tips.add(oid);
  }
  const upstreamOids = new Set<string>();
  for (const oid of tips) {
    try {
      for (const entry of await git.log({ fs, ...repo, ref: oid }))
        upstreamOids.add(entry.oid);
    } catch (error) {
      if ((error as { code?: string }).code !== "NotFoundError") throw error;
    }
  }
  const common = (await git.log({ fs, ...repo, ref: original })).filter(
    (entry) => upstreamOids.has(entry.oid),
  );
  const ancestors = new Set(common.flatMap((entry) => entry.commit.parent));
  const bases = common.filter((entry) => !ancestors.has(entry.oid));
  return bases.length === 1 && tips.has(bases[0]!.oid)
    ? bases[0]!.oid
    : undefined;
}
export async function rebaseCommand(
  repo: Repository,
  target?: string,
  branch?: string,
  forkPoint?: string,
): Promise<MutationResult> {
  await assertSupportedModes(repo);
  if ((await rebaseInProgress(repo)) || (await mergeInProgress(repo)))
    throw new Error(
      "an integration is already in progress; continue or abort it first",
    );
  for (const name of [directory, "rebase-apply"]) {
    const existing = await fs.promises
      .lstat(path.join(repo.gitdir, name))
      .catch((error) => {
        if (error.code === "ENOENT") return undefined;
        throw error;
      });
    if (existing)
      throw new Error(
        "an external rebase is in progress; finish it with the Git implementation that started it",
      );
  }
  if (branch) {
    validRef(`refs/heads/${branch}`);
    await resolveCommitRef(repo, `refs/heads/${branch}`);
    await checkoutCommand(repo, undefined, false, branch, []);
  }
  const ref =
    (await git.currentBranch({ fs, ...repo, fullname: true })) ?? "HEAD";
  const useForkPoint = target === undefined;
  if (!target) {
    if (ref === "HEAD")
      throw new Error("specify an upstream when HEAD is detached");
    target = (await upstream(repo, ref.slice("refs/heads/".length)))?.ref;
    if (!target)
      throw new Error("no upstream configured; specify a rebase revision");
  }
  const original = await resolveCommitRef(repo, "HEAD"),
    onto = await resolveCommitRef(repo, target);
  if (useForkPoint) forkPoint = await rebaseForkPoint(repo, target, original);
  await assertRefSupportedModes(repo, onto);
  if ((await statusEntries(repo, [])).some((entry) => entry.code !== "??"))
    throw new Error("cannot rebase with unstaged or staged changes");
  const local = await git.log({ fs, ...repo, ref: original });
  const remote = await git.log({ fs, ...repo, ref: onto });
  await assertRebaseAttributes(
    repo,
    [...local, ...remote].map((entry) => entry.oid),
  );
  const remoteOids = new Set(remote.map((entry) => entry.oid));
  const localOids = new Set(local.map((entry) => entry.oid));
  if (
    localOids.has(onto) &&
    (!forkPoint || !localOids.has(forkPoint) || remoteOids.has(forkPoint))
  )
    return { summary: "Already up to date", paths: [] };
  let boundary = remoteOids;
  if (forkPoint && localOids.has(forkPoint))
    boundary = new Set([
      ...remoteOids,
      ...(await git.log({ fs, ...repo, ref: forkPoint })).map(
        (entry) => entry.oid,
      ),
    ]);
  const candidates = local.filter((entry) => !boundary.has(entry.oid));
  if (candidates.some((entry) => entry.commit.parent.length > 1))
    throw new Error(
      "rebasing merge commits is not supported; no index or worktree changes were made",
    );
  const opposite = remote
    .filter((entry) => !localOids.has(entry.oid))
    .map((entry) => entry.oid);
  const localHeaders = await patchIds(
    repo,
    candidates.map((entry) => entry.oid),
    true,
  );
  const remoteHeaders = await patchIds(repo, opposite, true);
  const wantedHeaders = new Set(localHeaders.values());
  const possibleHeaders = new Set(remoteHeaders.values());
  const upstreamPatches = new Set(
    (
      await patchIds(
        repo,
        opposite.filter((oid) => wantedHeaders.has(remoteHeaders.get(oid)!)),
      )
    ).values(),
  );
  const localPatches = await patchIds(
    repo,
    candidates
      .map((entry) => entry.oid)
      .filter((oid) => possibleHeaders.has(localHeaders.get(oid)!)),
  );
  const todo = candidates
    .reverse()
    .filter((entry) => !upstreamPatches.has(localPatches.get(entry.oid)!))
    .map((entry) => entry.oid);
  // Validate every replay before creating persistent state or detaching HEAD.
  for (const oid of todo) await assertRefSupportedModes(repo, oid);
  if (todo.length) await localIdentity(repo);
  const head = await treeSnapshotMap(repo, original);
  const prepared = await prepareIndexAndWorktree(
    repo,
    head,
    { files: await treeSnapshotMap(repo, onto), conflicts: new Map() },
    true,
  );
  const state: State = {
    phase: "ready",
    original,
    ref,
    onto,
    todo,
    position: 0,
    current: original,
  };
  await install(repo, state, {
    before: original,
    after: onto,
    index: prepared.nextIndex,
    changes: prepared.changes,
    hard: false,
    next: "ready",
    advance: false,
  });
  return replay(repo, state);
}
export async function commitRebaseResolution(
  repo: Repository,
  message?: string,
  author?: { name: string; email: string },
  allowEmpty = false,
): Promise<string> {
  const state = await stateFor(repo);
  if (!state || state.phase !== "conflict")
    throw new Error("no stopped rebase is in progress");
  await verify(repo, state);
  if (state.resolved)
    throw new Error(
      "rebase resolution already committed; use rebase --continue",
    );
  const { commit } = await git.readCommit({
    fs,
    ...repo,
    oid: state.todo[state.position]!,
  });
  const identity = await localIdentity(repo);
  const result = await git.commit({
    fs,
    ...repo,
    parent: [state.current],
    message: message ?? commit.message,
    author: author ?? (message === undefined ? commit.author : identity),
    committer: identity,
    noUpdateBranch: true,
    disallowEmpty: !allowEmpty,
  });
  state.resolved = result;
  await install(repo, state, {
    before: state.current,
    after: result,
    changes: [],
    hard: false,
    next: "conflict",
    advance: false,
  });
  return result;
}
export async function continueRebase(
  repo: Repository,
): Promise<MutationResult> {
  let state = await stateFor(repo);
  if (!state) throw new Error("no rebase is in progress");
  await verify(repo, state);
  if (state.phase === "conflict") {
    if ((await conflictEntries(repo)).size)
      throw new Error(
        "resolve and add all conflicted paths before continuing rebase",
      );
    const changes = await statusEntries(repo, []);
    if (
      changes.some(
        (entry) => entry.code !== "??" && entry.worktree !== "unmodified",
      )
    )
      throw new Error(
        "unstaged changes prevent rebase continuation; add the resolved paths first",
      );
    if (!state.resolved && changes.some((entry) => entry.code !== "??"))
      await commitRebaseResolution(repo);
    state = (await stateFor(repo))!;
    state.position++;
    delete state.resolved;
    state.phase = "ready";
    await save(repo, state);
    await metadata(repo, state);
  }
  return replay(repo, state);
}
export async function skipRebase(repo: Repository): Promise<MutationResult> {
  const state = await stateFor(repo);
  if (!state || state.phase !== "conflict")
    throw new Error("no stopped rebase is in progress");
  await verify(repo, state);
  const parent = state.current;
  const prepared = await prepareIndexAndWorktree(
    repo,
    await treeSnapshotMap(repo, state.current),
    { files: await treeSnapshotMap(repo, parent), conflicts: new Map() },
    true,
    true,
  );
  delete state.resolved;
  await install(repo, state, {
    before: state.current,
    after: parent,
    index: prepared.nextIndex,
    changes: prepared.changes,
    hard: true,
    next: "ready",
    advance: true,
  });
  return replay(repo, state);
}
export async function abortRebase(repo: Repository): Promise<MutationResult> {
  const state = await stateFor(repo);
  if (!state) throw new Error("no rebase is in progress");
  await verify(repo, state);
  const prepared = await prepareIndexAndWorktree(
    repo,
    await treeSnapshotMap(repo, state.current),
    {
      files: await treeSnapshotMap(repo, state.original),
      conflicts: new Map(),
    },
    true,
    true,
  );
  await install(repo, state, {
    before: state.current,
    after: state.original,
    index: prepared.nextIndex,
    changes: prepared.changes,
    hard: true,
    next: "finish",
    advance: false,
  });
  return {
    summary: "Rebase aborted",
    paths: prepared.changes.map((change) => change.path),
  };
}
