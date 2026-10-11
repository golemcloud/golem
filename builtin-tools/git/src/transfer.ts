import fs from "node:fs";
import {
  lstat,
  mkdir,
  readFile,
  readdir,
  rename,
  rmdir,
  rm,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import * as git from "isomorphic-git";
import { GitRefManager } from "isomorphic-git/managers";
import { FileSystem } from "isomorphic-git/models";
import type { MutationResult, Repository } from "./git.ts";
import { assertRefSupportedModes, initializeRepository } from "./git.ts";
import {
  httpTransport,
  httpUrl,
  redirectPolicy,
  refspecDestination,
  remoteAdd,
  remoteNames,
  remoteUrls,
} from "./remote.ts";
import type { Credentials } from "./remote.ts";

const gitFs = new FileSystem(fs);
const journal = "golem-fetch-state";
type Ref = { ref: string; oid: string; target?: string; peeled?: string };
type Selection = {
  source: string;
  oid: string;
  destination?: string;
  force: boolean;
  merge: boolean;
};
type Publication = {
  writes: [string, string][];
  deletions: string[];
  logs: [string, string][];
  phase: "deleting" | "writing";
  packedRefs?: string;
};
export type FetchOptions = {
  all?: boolean;
  prune?: boolean;
  tags?: boolean;
  depth?: number;
  deepen?: number;
  unshallow?: boolean;
  append?: boolean;
  writeFetchHead?: boolean;
  force?: boolean;
};

export function validRef(ref: string): void {
  if (
    !ref.startsWith("refs/") ||
    /[\x00-\x20\x7f~^:?*\[\\]/.test(ref) ||
    ref.includes("..") ||
    ref.includes("@{") ||
    ref
      .split("/")
      .some(
        (part) =>
          !part ||
          part.startsWith(".") ||
          part.endsWith(".") ||
          part.endsWith(".lock"),
      )
  ) {
    throw new Error(`invalid reference name: ${ref}`);
  }
}

async function optionalFile(file: string): Promise<string | undefined> {
  try {
    return await readFile(file, "utf8");
  } catch (error) {
    if ((error as { code?: string }).code === "ENOENT") return undefined;
    throw error;
  }
}

async function atomicWrite(
  repo: Repository,
  name: string,
  value: string,
): Promise<void> {
  if (
    ![
      "HEAD",
      "logs/HEAD",
      "shallow",
      "FETCH_HEAD",
      "packed-refs",
      journal,
    ].includes(name)
  )
    validRef(name.replace(/^logs\//, ""));
  const target = path.join(repo.gitdir, name);
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(`${target}.lock`, value);
  await rename(`${target}.lock`, target);
}

export async function recoverFetch(repo: Repository): Promise<void> {
  const pending = await optionalFile(path.join(repo.gitdir, journal));
  if (pending === undefined) return;
  const state: Publication = JSON.parse(pending);
  if (!Array.isArray(state.logs))
    throw new Error("invalid reference publication logs");
  for (const [ref, value] of state.logs) {
    if (ref !== "HEAD") validRef(ref);
    if (typeof value !== "string")
      throw new Error("invalid reference publication log");
  }
  for (const [ref] of state.writes) {
    if (!["HEAD", "shallow", "FETCH_HEAD"].includes(ref)) validRef(ref);
  }
  for (const ref of state.deletions) {
    if (ref !== "shallow") validRef(ref);
  }
  if (state.phase !== "deleting" && state.phase !== "writing")
    throw new Error("invalid fetch publication phase");
  if (state.phase === "deleting") {
    for (const ref of state.deletions) {
      await rm(path.join(repo.gitdir, ref), { force: true });
      if (ref === "shallow") continue;
      await rm(path.join(repo.gitdir, "logs", ref), { force: true });
      let logParent = path.dirname(path.join(repo.gitdir, "logs", ref));
      while (logParent !== path.join(repo.gitdir, "logs/refs")) {
        try {
          await rmdir(logParent);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ENOTEMPTY") break;
          if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
        }
        logParent = path.dirname(logParent);
      }
      let parent = path.dirname(path.join(repo.gitdir, ref));
      while (parent !== path.join(repo.gitdir, "refs")) {
        try {
          await rmdir(parent);
        } catch (error) {
          const code = (error as { code?: string }).code;
          if (code === "ENOTEMPTY") break;
          if (code !== "ENOENT") throw error;
        }
        parent = path.dirname(parent);
      }
    }
    if (state.packedRefs !== undefined)
      await atomicWrite(repo, "packed-refs", state.packedRefs);
    state.phase = "writing";
    await atomicWrite(repo, journal, JSON.stringify(state));
  }
  for (const [ref, value] of state.writes) await atomicWrite(repo, ref, value);
  for (const [ref, value] of state.logs)
    await atomicWrite(repo, `logs/${ref}`, value);
  await rm(path.join(repo.gitdir, journal));
}

export async function publishRefs(
  repo: Repository,
  changes: Pick<Publication, "writes" | "deletions">,
): Promise<void> {
  const state: Publication = { ...changes, logs: [], phase: "deleting" };
  const settings = await git.getConfigAll({
    fs,
    ...repo,
    path: "core.logallrefupdates",
  });
  const setting = settings[settings.length - 1];
  const always = setting === "always";
  const recordLogs = always || bool(setting, true);
  const headRef =
    (await git.currentBranch({ fs, ...repo, fullname: true })) ?? "HEAD";
  for (const [ref, value] of state.writes) {
    if (
      (ref !== "HEAD" && !ref.startsWith("refs/")) ||
      !/^[0-9a-f]{40}\n$/.test(value)
    )
      continue;
    const existing = await optionalFile(
      path.join(repo.gitdir, "logs", ref),
    ).catch((error) => {
      if (
        ["ENOTDIR", "EISDIR"].includes(error.code) &&
        state.deletions.some(
          (other) => ref.startsWith(`${other}/`) || other.startsWith(`${ref}/`),
        )
      )
        return undefined;
      throw error;
    });
    if (
      existing === undefined &&
      !(
        always ||
        (recordLogs && (ref === "HEAD" || /^refs\/(heads|remotes)\//.test(ref)))
      )
    )
      continue;
    const previous = await oid(repo, ref);
    if (previous === value.trim()) continue;
    const name = String(
      (await git.getConfig({ fs, ...repo, path: "user.name" })) ?? "Golem",
    ).replace(/[<>\r\n\0]/g, "");
    const email = String(
      (await git.getConfig({ fs, ...repo, path: "user.email" })) ??
        "golem@localhost",
    ).replace(/[<>\r\n\0]/g, "");
    const entry = `${previous ?? "0".repeat(40)} ${value.trim()} ${name} <${email}> ${Math.floor(Date.now() / 1000)} +0000\tupdate by Git built-in tool\n`;
    state.logs.push([ref, `${existing ?? ""}${entry}`]);
    if (ref !== "HEAD" && ref === headRef)
      state.logs.push([
        "HEAD",
        `${(await optionalFile(path.join(repo.gitdir, "logs/HEAD"))) ?? ""}${entry}`,
      ]);
  }
  const refs = (
    await GitRefManager.listRefs({
      fs: gitFs,
      gitdir: repo.gitdir,
      filepath: "refs",
    })
  )
    .map((ref) => `refs/${ref}`)
    .filter((ref) => !state.deletions.includes(ref));
  for (const [ref] of state.writes) {
    if (!ref.startsWith("refs/")) continue;
    if (
      [...refs, ...state.writes.map(([name]) => name)].some(
        (other) =>
          other !== ref &&
          (ref.startsWith(`${other}/`) || other.startsWith(`${ref}/`)),
      )
    )
      throw new Error(`reference namespace conflict: ${ref}`);
  }
  const packed = await optionalFile(path.join(repo.gitdir, "packed-refs"));
  if (packed !== undefined) {
    let removed = false;
    state.packedRefs = packed
      .split("\n")
      .filter((line) => {
        if (line.startsWith("^")) return !removed;
        removed = state.deletions.includes(line.split(" ")[1] ?? "");
        return !removed;
      })
      .join("\n");
  }
  await atomicWrite(repo, journal, JSON.stringify(state));
  await recoverFetch(repo);
}

async function oid(repo: Repository, ref: string): Promise<string | undefined> {
  try {
    return await git.resolveRef({ fs, ...repo, ref });
  } catch (error) {
    if ((error as { code?: string }).code === "NotFoundError") return undefined;
    throw error;
  }
}

function bool(value: unknown, fallback = false): boolean {
  if (value === undefined) return fallback;
  if (typeof value === "boolean") return value;
  if (["true", "yes", "on", "1"].includes(String(value).toLowerCase()))
    return true;
  if (["false", "no", "off", "0"].includes(String(value).toLowerCase()))
    return false;
  throw new Error(`invalid boolean configuration value: ${String(value)}`);
}

function sourceRef(refs: Ref[], name: string): Ref {
  const found =
    refs.find((ref) => ref.ref === name) ??
    refs.find((ref) => ref.ref === `refs/heads/${name}`) ??
    refs.find((ref) => ref.ref === `refs/tags/${name}`);
  if (found) return found;
  if (/^[0-9a-f]{40}$/.test(name)) return { ref: name, oid: name };
  throw new Error(`remote reference does not exist: ${name}`);
}

function excludes(specs: string[], ref: string): boolean {
  return specs
    .filter((spec) => spec.startsWith("^"))
    .some(
      (spec) =>
        refspecDestination(`${spec.slice(1)}:${spec.slice(1)}`, ref) !==
        undefined,
    );
}

function select(
  refs: Ref[],
  specs: string[],
  mergeRefs: string[],
): Selection[] {
  const selected: Selection[] = [];
  for (const spec of specs) {
    if (spec.startsWith("^")) {
      if (spec.includes(":"))
        throw new Error(`invalid negative refspec: ${spec}`);
      continue;
    }
    const force = spec.startsWith("+");
    const [source, destination, extra] = spec.replace(/^\+/, "").split(":");
    if (!source || extra !== undefined)
      throw new Error(`invalid fetch refspec: ${spec}`);
    if (source.includes("*")) {
      if (!destination)
        throw new Error(
          `wildcard fetch refspec requires a destination: ${spec}`,
        );
      for (const ref of refs) {
        const target = refspecDestination(spec, ref.ref);
        if (target && !excludes(specs, ref.ref)) {
          validRef(target);
          selected.push({
            source: ref.ref,
            oid: ref.oid,
            destination: target,
            force,
            merge: mergeRefs.includes(ref.ref),
          });
        }
      }
    } else {
      const ref = sourceRef(refs, source);
      if (excludes(specs, ref.ref)) continue;
      if (destination) validRef(destination);
      selected.push({
        source: ref.ref,
        oid: ref.oid,
        destination: destination || undefined,
        force,
        merge: mergeRefs.includes(ref.ref),
      });
    }
  }
  return selected;
}

async function validateGraph(
  repo: Repository,
  roots: string[],
  shallows: Set<string>,
): Promise<Set<string>> {
  const seen = new Set<string>();
  const cache = {};
  type ObjectType = "commit" | "tree" | "blob" | "tag";
  const queue: { oid: string; type?: ObjectType }[] = roots.map((oid) => ({
    oid,
  }));
  queue.push(...[...shallows].map((oid) => ({ oid, type: "commit" as const })));
  while (queue.length) {
    const { oid: current, type } = queue.pop()!;
    if (!/^[0-9a-f]{40}$/.test(current))
      throw new Error("invalid object ID in fetched graph");
    const object = await git.readObject({
      fs,
      ...repo,
      oid: current,
      cache,
      format: "content",
    });
    if (!["commit", "tree", "blob", "tag"].includes(object.type))
      throw new Error("invalid fetched object type");
    if (
      (await git.writeObject({
        fs,
        ...repo,
        type: object.type as ObjectType,
        object: object.object,
        format: "content",
      })) !== current
    )
      throw new Error("fetched object checksum mismatch");
    if (type && object.type !== type)
      throw new Error(
        `invalid fetched object type: expected ${type}, got ${object.type}`,
      );
    if (seen.has(current)) continue;
    seen.add(current);
    if (object.type === "commit") {
      const { commit } = await git.readCommit({
        fs,
        ...repo,
        oid: current,
        cache,
      });
      queue.push({ oid: commit.tree, type: "tree" });
      if (!shallows.has(current))
        queue.push(
          ...commit.parent.map((oid: string) => ({
            oid,
            type: "commit" as const,
          })),
        );
    } else if (object.type === "tree") {
      const { tree } = await git.readTree({ fs, ...repo, oid: current, cache });
      for (const entry of tree) {
        // Gitlinks name commits in another repository, not missing local objects.
        if (entry.mode === "160000") continue;
        const type =
          entry.mode === "040000"
            ? "tree"
            : ["100644", "100755", "120000"].includes(entry.mode)
              ? "blob"
              : undefined;
        if (!type) throw new Error(`invalid fetched tree mode: ${entry.mode}`);
        queue.push({ oid: entry.oid, type });
      }
    } else if (object.type === "tag") {
      const { tag } = await git.readTag({ fs, ...repo, oid: current, cache });
      if (!["commit", "tree", "blob", "tag"].includes(tag.type))
        throw new Error("invalid annotated tag target type");
      queue.push({ oid: tag.object, type: tag.type as ObjectType });
    }
  }
  return seen;
}

export async function copyObjects(
  source: string,
  target: string,
): Promise<void> {
  await mkdir(target, { recursive: true });
  for (const name of await readdir(source).catch((error) => {
    if (error.code === "ENOENT") return [] as string[];
    throw error;
  })) {
    const from = path.join(source, name),
      to = path.join(target, name);
    const metadata = await lstat(from);
    if (metadata.isSymbolicLink())
      throw new Error("symbolic links in the object database are unsupported");
    if (metadata.isDirectory()) await copyObjects(from, to);
    else if (metadata.isFile()) {
      await writeFile(`${to}.lock`, await readFile(from));
      await rename(`${to}.lock`, to);
    } else throw new Error("non-regular object database entry");
  }
}

// Upstream fetch publishes its refs before finishing the pack. Keep those effects
// in a disposable repository, then import objects before publishing application refs.
async function download(
  repo: Repository,
  http: git.HttpClient,
  url: string,
  roots: { source: string; oid: string }[],
  shallow: Set<string>,
  options: FetchOptions,
): Promise<void> {
  const gitdir = await fs.promises.mkdtemp(
    path.join(repo.gitdir, "golem-download-"),
  );
  const staged = { ...repo, gitdir };
  try {
    await git.init({ fs, ...staged });
    await copyObjects(
      path.join(repo.gitdir, "objects"),
      path.join(gitdir, "objects"),
    );
    const boundaries = new Set(shallow);
    await git.setConfig({
      fs,
      ...staged,
      path: "remote.download.fetch",
      value: "+refs/*:refs/golem-download/*",
    });
    for (const root of new Map(
      roots.map((root) => [root.source, root]),
    ).values()) {
      // Relative depth is measured from the caller's boundaries, not from an
      // earlier selected ref's download. Combine additions/removals afterwards.
      if (shallow.size)
        await writeFile(
          path.join(gitdir, "shallow"),
          [...shallow].join("\n") + "\n",
        );
      else await rm(path.join(gitdir, "shallow"), { force: true });
      const result = await git.fetch({
        fs,
        ...staged,
        http,
        url,
        remote: "download",
        ref: root.source,
        remoteRef: root.source,
        singleBranch: true,
        tags: false,
        depth: options.unshallow
          ? 2147483647
          : (options.deepen ?? options.depth),
        relative: options.deepen !== undefined,
      });
      if (result.fetchHead !== root.oid)
        throw new Error(
          "remote reference changed during fetch; application refs were not published",
        );
      const fetched = new Set(
        ((await optionalFile(path.join(gitdir, "shallow"))) ?? "")
          .trim()
          .split("\n")
          .filter(Boolean),
      );
      for (const original of shallow)
        if (!fetched.has(original)) boundaries.delete(original);
      for (const added of fetched)
        if (!shallow.has(added)) boundaries.add(added);
    }
    await validateGraph(
      staged,
      roots.map((root) => root.oid),
      boundaries,
    );
    await copyObjects(
      path.join(gitdir, "objects"),
      path.join(repo.gitdir, "objects"),
    );
    shallow.clear();
    for (const boundary of boundaries) shallow.add(boundary);
  } finally {
    await rm(gitdir, { recursive: true, force: true });
  }
}

export async function fetchCommand(
  repo: Repository,
  remote: string | undefined,
  explicitSpecs: string[],
  options: FetchOptions,
  send: typeof fetch,
  credentials?: Credentials,
): Promise<MutationResult> {
  await recoverFetch(repo);
  if (
    [
      options.depth !== undefined,
      options.deepen !== undefined,
      !!options.unshallow,
    ].filter(Boolean).length > 1
  ) {
    throw new Error(
      "--depth, --deepen, and --unshallow are mutually exclusive",
    );
  }
  for (const value of [options.depth, options.deepen]) {
    if (value !== undefined && (!Number.isSafeInteger(value) || value <= 0))
      throw new Error("depth must be a positive integer");
  }
  const shallowFile =
    (await optionalFile(path.join(repo.gitdir, "shallow"))) ?? "";
  if (options.unshallow && !shallowFile.trim())
    throw new Error("--unshallow on a complete repository does not make sense");
  if (options.all) {
    if (remote || explicitSpecs.length)
      throw new Error("--all does not accept a remote or refspecs");
    const results = [];
    for (const name of await remoteNames(repo)) {
      results.push(
        await fetchCommand(
          repo,
          name,
          [],
          {
            ...options,
            all: false,
            append: options.append || results.length > 0,
          },
          send,
          credentials,
        ),
      );
    }
    return {
      summary: `Fetched ${results.length} remotes`,
      paths: [] as string[],
    };
  }
  const branch = await git.currentBranch({ fs, ...repo });
  const names = await remoteNames(repo);
  remote ??= String(
    (branch
      ? await git.getConfig({ fs, ...repo, path: `branch.${branch}.remote` })
      : undefined) ?? "origin",
  );
  const configured = names.includes(remote);
  const url = configured ? (await remoteUrls(repo, remote))[0] : remote;
  if (!url) throw new Error("remote has no fetch URL");
  httpUrl(url);
  const specs = explicitSpecs.length
    ? [...explicitSpecs]
    : configured
      ? await git.getConfigAll({ fs, ...repo, path: `remote.${remote}.fetch` })
      : ["HEAD"];
  const fallbackHead = !explicitSpecs.length && (!configured || !specs.length);
  if (!specs.length) specs.push("HEAD");
  const mergeRefs: string[] = explicitSpecs.length
    ? []
    : configured &&
        branch &&
        (await git.getConfig({
          fs,
          ...repo,
          path: `branch.${branch}.remote`,
        })) === remote
      ? await git.getConfigAll({ fs, ...repo, path: `branch.${branch}.merge` })
      : [];
  const prune =
    options.prune ??
    bool(
      configured
        ? await git.getConfig({ fs, ...repo, path: `remote.${remote}.prune` })
        : undefined,
      bool(await git.getConfig({ fs, ...repo, path: "fetch.prune" })),
    );
  const tagSetting = configured
    ? await git.getConfig({ fs, ...repo, path: `remote.${remote}.tagopt` })
    : undefined;
  if (
    tagSetting !== undefined &&
    tagSetting !== "--tags" &&
    tagSetting !== "--no-tags"
  )
    throw new Error("invalid remote tagOpt setting");
  const tags =
    options.tags ??
    (tagSetting === "--tags"
      ? true
      : tagSetting === "--no-tags"
        ? false
        : undefined);
  if (
    prune &&
    bool(await git.getConfig({ fs, ...repo, path: "fetch.pruneTags" }))
  )
    specs.push("refs/tags/*:refs/tags/*");
  return httpTransport(credentials, send, await redirectPolicy(repo)).run(
    async (http) => {
      const refs: Ref[] = await git.listServerRefs({
        http,
        url,
        symrefs: true,
        peelTags: true,
      });
      for (const ref of refs) {
        if (ref.ref !== "HEAD") validRef(ref.ref);
        if (!/^[0-9a-f]{40}$/.test(ref.oid))
          throw new Error("invalid advertised object ID");
      }
      const selection = select(refs, specs, mergeRefs);
      for (const mergeRef of mergeRefs) {
        if (
          !selection.some((entry) => entry.source === mergeRef) &&
          !excludes(specs, mergeRef)
        )
          selection.push(...select(refs, [mergeRef], mergeRefs));
      }
      if (fallbackHead) for (const entry of selection) entry.merge = true;
      if (explicitSpecs.length)
        for (const entry of selection) entry.merge = true;
      if (tags === true) {
        const tagSpec = "refs/tags/*:refs/tags/*";
        selection.push(
          ...select(
            refs,
            [tagSpec, ...specs.filter((spec) => spec.startsWith("^"))],
            [],
          ),
        );
      }
      // Explicit fetches also update the configured tracking mapping, without
      // adding other configured sources to FETCH_HEAD or the request.
      if (explicitSpecs.length && configured) {
        const mappings = await git.getConfigAll({
          fs,
          ...repo,
          path: `remote.${remote}.fetch`,
        });
        for (const entry of [...selection]) {
          if (excludes(mappings, entry.source)) continue;
          for (const mapping of mappings) {
            const target = refspecDestination(mapping, entry.source);
            if (target) {
              validRef(target);
              selection.push({
                ...entry,
                destination: target,
                force: mapping.startsWith("+"),
                merge: entry.merge,
              });
            }
          }
        }
      }
      const shallow = new Set(shallowFile.trim().split("\n").filter(Boolean));
      if (selection.length) {
        await download(repo, http, url, selection, shallow, options);
      }
      const reachable = await validateGraph(
        repo,
        selection.map((entry) => entry.oid),
        shallow,
      );
      if (
        tags === undefined &&
        specs.some((spec) => !spec.startsWith("^") && !!spec.split(":")[1])
      ) {
        const candidates = refs.filter(
          (ref) =>
            ref.ref.startsWith("refs/tags/") &&
            !excludes(specs, ref.ref) &&
            reachable.has(ref.peeled ?? ref.oid) &&
            !selection.some((entry) => entry.source === ref.ref),
        );
        const following: Ref[] = [];
        for (const ref of candidates) {
          if ((await oid(repo, ref.ref)) === undefined) following.push(ref);
        }
        const missing = following.filter((ref) => !reachable.has(ref.oid));
        if (missing.length)
          await download(
            repo,
            http,
            url,
            missing.map((ref) => ({ source: ref.ref, oid: ref.oid })),
            shallow,
            {},
          );
        await validateGraph(
          repo,
          following.map((ref) => ref.oid),
          shallow,
        );
        for (const ref of following)
          selection.push({
            source: ref.ref,
            destination: ref.ref,
            oid: ref.oid,
            force: false,
            merge: false,
          });
      }
      const writes = new Map<string, string>();
      const rejected: string[] = [];
      const current = branch ? `refs/heads/${branch}` : undefined;
      for (const entry of selection) {
        const destination = entry.destination;
        if (!destination) continue;
        if (destination === current)
          throw new Error("refusing to fetch into the checked-out branch");
        if (
          destination.startsWith("refs/heads/") &&
          (await git.readObject({ fs, ...repo, oid: entry.oid })).type !==
            "commit"
        ) {
          rejected.push(`non-commit branch update rejected: ${destination}`);
          continue;
        }
        const prior = await oid(repo, destination);
        if (prior && prior !== entry.oid && !entry.force && !options.force) {
          if (destination.startsWith("refs/tags/")) {
            rejected.push(`tag update rejected: ${destination}`);
            continue;
          }
          if (
            destination.startsWith("refs/heads/") &&
            !(await git.isDescendent({
              fs,
              ...repo,
              oid: entry.oid,
              ancestor: prior,
            }))
          ) {
            rejected.push(`non-fast-forward fetch rejected: ${destination}`);
            continue;
          }
        }
        const value = `${entry.oid}\n`;
        if (writes.has(destination) && writes.get(destination) !== value)
          throw new Error(`conflicting fetch destinations: ${destination}`);
        writes.set(destination, value);
      }
      const deletions: string[] = [];
      if (prune) {
        const local = await GitRefManager.listRefs({
          fs: gitFs,
          gitdir: repo.gitdir,
          filepath: "refs",
        });
        for (const suffix of local) {
          const destination = `refs/${suffix}`;
          if (writes.has(destination)) continue;
          const value = await GitRefManager.resolve({
            fs: gitFs,
            gitdir: repo.gitdir,
            ref: destination,
            depth: 1,
          });
          if (value.startsWith("ref: ")) continue;
          const mappedSources: string[] = [];
          for (const spec of specs) {
            if (spec.startsWith("^")) continue;
            const [source, target] = spec.replace(/^\+/, "").split(":");
            if (!source || !target) continue;
            const mapped = refspecDestination(
              `${target}:${source}`,
              destination,
            );
            if (mapped && !excludes(specs, mapped)) mappedSources.push(mapped);
          }
          if (
            mappedSources.length &&
            !mappedSources.some((source) =>
              refs.some((ref) => ref.ref === source),
            )
          ) {
            if (destination === current)
              throw new Error("refusing to prune the checked-out branch");
            deletions.push(destination);
          }
        }
      }
      if (shallow.size)
        writes.set("shallow", [...shallow].sort().join("\n") + "\n");
      else if (shallowFile) deletions.push("shallow");
      if (options.writeFetchHead !== false) {
        const entries = new Map<string, Selection>();
        for (const entry of selection) entries.set(entry.source, entry);
        const lines = [...entries.values()]
          .sort(
            (a, b) =>
              Number(b.merge) - Number(a.merge) ||
              a.source.localeCompare(b.source),
          )
          .map((entry) => {
            const noun = entry.source.startsWith("refs/tags/")
              ? "tag"
              : entry.source.startsWith("refs/heads/")
                ? "branch"
                : "ref";
            const name = entry.source.replace(/^refs\/(heads|tags)\//, "");
            return `${entry.oid}\t${entry.merge ? "" : "not-for-merge"}\t${noun} '${name}' of ${url}\n`;
          })
          .join("");
        writes.set(
          "FETCH_HEAD",
          (options.append
            ? ((await optionalFile(path.join(repo.gitdir, "FETCH_HEAD"))) ?? "")
            : "") + lines,
        );
      }
      const head = refs.find((ref) => ref.ref === "HEAD");
      if (
        configured &&
        !explicitSpecs.length &&
        !fallbackHead &&
        head?.target?.startsWith("refs/heads/")
      ) {
        const target = `refs/remotes/${remote}/${head.target.slice(11)}`;
        const symbolic = `refs/remotes/${remote}/HEAD`;
        if (
          !(await oid(repo, symbolic)) &&
          (writes.has(target) || (await oid(repo, target)))
        ) {
          writes.set(symbolic, `ref: ${target}\n`);
        }
      }
      await publishRefs(repo, {
        writes: [...writes],
        deletions: [...new Set(deletions)],
      });
      if (rejected.length) throw new Error(rejected.join("; "));
      return {
        summary: `Fetched ${new Set(selection.map((entry) => entry.source)).size} references from ${remote}`,
        paths: [] as string[],
      };
    },
  );
}

export type CloneOptions = {
  origin?: string;
  branch?: string;
  noCheckout?: boolean;
  singleBranch?: boolean;
  depth?: number;
  tags?: boolean;
};
export async function cloneCommand(
  cwd: string,
  url: string,
  directory: string | undefined,
  options: CloneOptions,
  send: typeof fetch,
  credentials?: Credentials,
) {
  const parsed = httpUrl(url);
  const basename = parsed.pathname
    .replace(/\/+$/, "")
    .split("/")
    .pop()
    ?.replace(/\.git$/, "");
  if (!directory && !basename)
    throw new Error("cannot determine destination directory; specify one");
  const dir = path.resolve(cwd, directory ?? basename!);
  try {
    const metadata = await lstat(dir);
    if (
      !metadata.isDirectory() ||
      metadata.isSymbolicLink() ||
      (await readdir(dir)).length
    )
      throw new Error(
        "clone destination must be a missing or empty regular directory",
      );
  } catch (error) {
    if ((error as { code?: string }).code !== "ENOENT") throw error;
  }
  if (
    options.depth !== undefined &&
    (!Number.isSafeInteger(options.depth) || options.depth <= 0)
  )
    throw new Error("depth must be a positive integer");
  const refs = await httpTransport(credentials, send).run((http) =>
    git.listServerRefs({
      http,
      url,
      symrefs: true,
      peelTags: true,
    }),
  );
  for (const ref of refs) {
    if (ref.ref !== "HEAD") validRef(ref.ref);
    if (ref.target) validRef(ref.target);
    if (
      !/^[0-9a-f]{40}$/.test(ref.oid) &&
      !(ref.ref === "HEAD" && ref.oid === "unborn" && ref.target)
    )
      throw new Error("invalid advertised object ID");
  }
  const requested = options.branch
    ? sourceRef(refs, options.branch)
    : refs.find((ref) => ref.ref === "HEAD");
  const unborn = requested?.oid === "unborn" ? requested.target : undefined;
  const selected = unborn
    ? undefined
    : requested?.target
      ? sourceRef(refs, requested.target)
      : requested;
  const origin = options.origin ?? "origin";
  const single = options.singleBranch ?? options.depth !== undefined;
  const defaultBranch = selected?.ref.startsWith("refs/heads/")
    ? selected.ref.slice(11)
    : unborn?.startsWith("refs/heads/")
      ? unborn.slice(11)
      : "master";
  await initializeRepository(dir, defaultBranch);
  const repo = { dir, cwd: dir, gitdir: path.join(dir, ".git") };
  await remoteAdd(repo, origin, url);
  if (single) {
    await git.setConfig({
      fs,
      ...repo,
      path: `remote.${origin}.fetch`,
      value: selected?.ref.startsWith("refs/heads/")
        ? `+${selected.ref}:refs/remotes/${origin}/${selected.ref.slice(11)}`
        : selected?.ref.startsWith("refs/tags/")
          ? `+${selected.ref}:${selected.ref}`
          : unborn
            ? `+${unborn}:refs/remotes/${origin}/${defaultBranch}`
            : undefined,
    });
  }
  if (options.tags === false)
    await git.setConfig({
      fs,
      ...repo,
      path: `remote.${origin}.tagopt`,
      value: "--no-tags",
    });
  if (selected?.ref.startsWith("refs/heads/") || unborn) {
    await git.setConfig({
      fs,
      ...repo,
      path: `branch.${defaultBranch}.remote`,
      value: origin,
    });
    await git.setConfig({
      fs,
      ...repo,
      path: `branch.${defaultBranch}.merge`,
      value: selected?.ref ?? unborn,
    });
  }
  if (refs.some((ref) => ref.oid !== "unborn")) {
    const explicit =
      selected && !selected.ref.startsWith("refs/heads/")
        ? selected.ref.startsWith("refs/tags/")
          ? `${selected.ref}:${selected.ref}`
          : selected.ref
        : selected?.ref;
    await fetchCommand(
      repo,
      origin,
      single && explicit
        ? [
            selected?.ref.startsWith("refs/heads/")
              ? `+${selected.ref}:refs/remotes/${origin}/${selected.ref.slice(11)}`
              : explicit,
          ]
        : selected && !selected.ref.startsWith("refs/heads/")
          ? [`+refs/heads/*:refs/remotes/${origin}/*`, explicit!]
          : [],
      { depth: options.depth, tags: options.tags, writeFetchHead: false },
      send,
      credentials,
    );
    const head = refs.find((ref) => ref.ref === "HEAD");
    if (
      head?.target?.startsWith("refs/heads/") &&
      (await oid(repo, `refs/remotes/${origin}/${head.target.slice(11)}`))
    ) {
      await atomicWrite(
        repo,
        `refs/remotes/${origin}/HEAD`,
        `ref: refs/remotes/${origin}/${head.target.slice(11)}\n`,
      );
    }
    if (selected) {
      let value = selected.ref.startsWith("refs/heads/")
        ? await oid(repo, `refs/remotes/${origin}/${defaultBranch}`)
        : selected.oid;
      if (!value) throw new Error("remote default branch was not fetched");
      if (
        selected.ref.startsWith("refs/heads/") &&
        (await git.readObject({ fs, ...repo, oid: value })).type !== "commit"
      )
        throw new Error("clone branch must name a commit");
      if (!selected.ref.startsWith("refs/heads/")) {
        for (;;) {
          const object = await git.readObject({ fs, ...repo, oid: value });
          if (object.type === "commit") break;
          if (object.type !== "tag")
            throw new Error(
              "clone branch must name a commit or tag of a commit",
            );
          const target: unknown = (
            await git.readTag({ fs, ...repo, oid: value })
          ).tag.object;
          if (typeof target !== "string" || !/^[0-9a-f]{40}$/.test(target))
            throw new Error("invalid annotated tag target");
          value = target;
        }
      }
      if (selected.ref.startsWith("refs/heads/"))
        await atomicWrite(repo, `refs/heads/${defaultBranch}`, `${value}\n`);
      else await writeFile(path.join(repo.gitdir, "HEAD"), `${value}\n`);
      if (!options.noCheckout) {
        await assertRefSupportedModes(repo, value);
        await git.checkout({
          fs,
          ...repo,
          ref: selected.ref.startsWith("refs/heads/") ? defaultBranch : value,
          force: true,
        });
      }
    }
  }
  return { summary: `Cloned ${url} into ${dir}`, paths: [dir] };
}
