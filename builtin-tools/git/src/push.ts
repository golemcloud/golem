import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import type { Repository } from "./git.ts";
import {
  httpTransport,
  httpUrl,
  redirectPolicy,
  refspecDestination,
  remoteNames,
  remoteUrls,
} from "./remote.ts";
import type { Credentials } from "./remote.ts";
import { copyObjects, publishRefs, validRef } from "./transfer.ts";

const zero = "0000000000000000000000000000000000000000";
export type PushOptions = {
  setUpstream?: boolean;
  delete?: boolean;
  tags?: boolean;
  all?: boolean;
  force?: boolean;
  forceWithLease?: boolean;
  lease?: string;
  atomic?: boolean;
};
type Update = { source?: string; ref: string; oid: string; force: boolean };

export async function pushCommand(
  repo: Repository,
  remote: string | undefined,
  explicit: string[],
  options: PushOptions,
  send: typeof fetch,
  credentials?: Credentials,
) {
  if (options.atomic)
    throw new Error(
      "atomic push is unsupported by released isomorphic-git; no push was sent",
    );
  if (options.all && (options.tags || options.delete || explicit.length))
    throw new Error(
      "--all does not combine with tags, deletion or explicit refspecs",
    );
  if (options.delete && (options.tags || !explicit.length))
    throw new Error(
      "--delete requires explicit reference names and cannot combine with --tags",
    );
  if (options.lease && !options.forceWithLease)
    throw new Error("--lease requires --force-with-lease");
  const branch = await git.currentBranch({ fs, ...repo });
  const names = await remoteNames(repo);
  const upstreamRemote = branch
    ? await git.getConfig({ fs, ...repo, path: `branch.${branch}.remote` })
    : undefined;
  remote ??= String(
    (branch
      ? await git.getConfig({
          fs,
          ...repo,
          path: `branch.${branch}.pushRemote`,
        })
      : undefined) ??
      (await git.getConfig({ fs, ...repo, path: "remote.pushDefault" })) ??
      upstreamRemote ??
      (names.length === 1 ? names[0] : "origin"),
  );
  const configured = names.includes(remote);
  const urls = configured ? await remoteUrls(repo, remote, true) : [remote];
  if (!urls.length) throw new Error("remote has no push URL");
  for (const url of urls) httpUrl(url);
  // Snapshot lease expectations before any remote request; never refresh them by fetching.
  const localRefs = new Map<string, string>();
  for (const suffix of await git.listRefs({ fs, ...repo, filepath: "refs" })) {
    const ref = `refs/${suffix}`;
    try {
      localRefs.set(ref, await git.resolveRef({ fs, ...repo, ref }));
    } catch (error) {
      if (
        (error as { code?: string }).code !== "NotFoundError" ||
        !(await git.resolveRef({ fs, ...repo, ref, depth: 1 })).startsWith(
          "ref: ",
        )
      )
        throw error;
    }
  }
  const mappings: string[] = configured
    ? await git.getConfigAll({ fs, ...repo, path: `remote.${remote}.fetch` })
    : [];
  const trackingRefs = (ref: string): string[] => {
    if (
      mappings.some(
        (mapping) =>
          mapping.startsWith("^") &&
          refspecDestination(`${mapping.slice(1)}:${mapping.slice(1)}`, ref),
      )
    )
      return [];
    return [
      ...new Set(
        mappings
          .filter((mapping) => !mapping.startsWith("^"))
          .flatMap((mapping) => {
            const target = refspecDestination(mapping, ref);
            return target ? [target] : [];
          }),
      ),
    ];
  };
  let specs = [...explicit];
  if (options.all) specs = ["refs/heads/*:refs/heads/*"];
  if (options.tags) specs.push("refs/tags/*:refs/tags/*");
  if (!specs.length && configured)
    specs = await git.getConfigAll({
      fs,
      ...repo,
      path: `remote.${remote}.push`,
    });
  if (!specs.length) {
    const mode = String(
      (await git.getConfig({ fs, ...repo, path: "push.default" })) ?? "simple",
    );
    if (mode === "nothing")
      throw new Error("push.default=nothing requires an explicit refspec");
    if (mode === "matching")
      throw new Error(
        "matching push is unsupported by released isomorphic-git; use a single explicit refspec",
      );
    if (!branch)
      throw new Error("detached HEAD requires an explicit push refspec");
    let destination = `refs/heads/${branch}`;
    if (
      mode === "upstream" ||
      (mode === "simple" &&
        remote ===
          (upstreamRemote ?? (names.length === 1 ? names[0] : "origin")))
    ) {
      const merge: string[] = await git.getConfigAll({
        fs,
        ...repo,
        path: `branch.${branch}.merge`,
      });
      if (!upstreamRemote || merge.length !== 1)
        throw new Error(
          "current branch has no single upstream; use an explicit refspec and --set-upstream",
        );
      if (remote !== upstreamRemote)
        throw new Error(
          "push.default=upstream cannot push to a different remote",
        );
      if (mode === "simple" && merge[0] !== destination)
        throw new Error(
          "push.default=simple requires the upstream branch to have the same name",
        );
      destination = merge[0]!;
    } else if (!["current", "simple"].includes(mode))
      throw new Error(`unsupported push.default: ${mode}`);
    specs = [`refs/heads/${branch}:${destination}`];
  }
  let leaseRef: string | undefined, leaseOid: string | undefined;
  if (options.lease !== undefined) {
    const parts = options.lease.split(":");
    if (
      parts.length !== 2 ||
      !parts[0] ||
      (parts[1] !== "" && !/^[0-9a-f]{40}$/.test(parts[1]!))
    )
      throw new Error(
        "lease must be a fully qualified reference followed by :OID or : for an absent reference",
      );
    leaseRef = parts[0];
    validRef(leaseRef);
    leaseOid = parts[1] || zero;
  }
  const selected = new Map<string, Update>();
  const add = (update: Update) => {
    validRef(
      update.ref.startsWith("refs/") ? update.ref : `refs/heads/${update.ref}`,
    );
    const prior = selected.get(update.ref);
    if (prior && (prior.oid !== update.oid || prior.source !== update.source))
      throw new Error(`conflicting push destination: ${update.ref}`);
    selected.set(update.ref, update);
  };
  for (const raw of specs) {
    if (raw.startsWith("^"))
      throw new Error("negative push refspecs are not supported by Git");
    const spec = raw.replace(/^\+/, ""),
      force = !!(options.force || raw.startsWith("+"));
    if (spec === ":")
      throw new Error(
        "matching push is unsupported by released isomorphic-git; use a single explicit refspec",
      );
    const [source, destination, extra] = spec.split(":");
    if (extra !== undefined || source === undefined || destination === "")
      throw new Error(`invalid push refspec: ${raw}`);
    if (source.includes("*") || destination?.includes("*")) {
      if (!source || !destination || options.delete)
        throw new Error(`invalid wildcard push refspec: ${raw}`);
      refspecDestination(spec, "");
      for (const [ref, oid] of localRefs) {
        const target = refspecDestination(spec, ref);
        if (target) add({ source: ref, ref: target, oid, force });
      }
      continue;
    }
    const deleting = options.delete || !source;
    if (options.delete && destination !== undefined)
      throw new Error(
        "--delete accepts reference names, not source:destination refspecs",
      );
    let local: string | undefined,
      oid = zero;
    if (!deleting) {
      const candidates = new Set([
        source,
        `refs/${source}`,
        `refs/tags/${source}`,
        `refs/heads/${source}`,
        `refs/remotes/${source}`,
        `refs/remotes/${source}/HEAD`,
      ]);
      const matches = [...localRefs.keys()].filter((ref) =>
        candidates.has(ref),
      );
      const strong = matches.filter(
        (ref) =>
          ref.length === source.length ||
          ref.length - 5 === source.length ||
          ref.startsWith("refs/heads/") ||
          ref.startsWith("refs/tags/"),
      );
      const refs = strong.length ? strong : matches;
      if (refs.length > 1)
        throw new Error(`src refspec ${source} matches more than one`);
      local =
        refs[0] ??
        (source === "HEAD" && branch
          ? `refs/heads/${branch}`
          : /^[0-9a-f]{40}$/.test(source)
            ? source
            : await git.expandRef({ fs, ...repo, ref: source }));
      oid = await git.resolveRef({ fs, ...repo, ref: local });
    }
    const target = options.delete ? source : destination || local;
    if (!target) throw new Error("deletion requires a destination");
    if (
      !target.startsWith("refs/") &&
      !deleting &&
      !local?.startsWith("refs/heads/") &&
      !local?.startsWith("refs/tags/")
    )
      throw new Error(
        "push of an object ID or detached HEAD requires a fully qualified destination",
      );
    add({ source: local, ref: target, oid, force });
  }
  if (selected.size > 1)
    throw new Error(
      "multi-ref push is unsupported by released isomorphic-git; no push was sent (use separate single-ref pushes, which are not atomic)",
    );
  const update = [...selected.values()][0];
  if (!update) return { summary: "Everything up-to-date", refs: [] };
  if (update.oid === zero) await git.resolveRef({ fs, ...repo, ref: "HEAD" });
  if (update.ref.startsWith("refs/heads/") && update.oid !== zero)
    await git.readCommit({ fs, ...repo, oid: update.oid });
  const results: {
    url: string;
    ref: string;
    oid: string;
    oldoid: string;
    ok: boolean;
    error?: string;
    upToDate: boolean;
  }[] = [];
  const failures: string[] = [];
  for (const url of urls) {
    let ref = update.ref,
      oldoid = zero,
      upToDate = false;
    try {
      const gitdir = await fs.promises.mkdtemp(
        path.join(repo.gitdir, "golem-push-"),
      );
      const staged = { ...repo, gitdir };
      try {
        await git.init({ fs, ...staged });
        await copyObjects(
          path.join(repo.gitdir, "objects"),
          path.join(gitdir, "objects"),
        );
        const shallow = await fs.promises
          .readFile(path.join(repo.gitdir, "shallow"))
          .catch((error) => {
            if (error.code === "ENOENT") return undefined;
            throw error;
          });
        if (shallow)
          await fs.promises.writeFile(path.join(gitdir, "shallow"), shallow);
        await git.writeRef({
          fs,
          ...staged,
          ref: "refs/heads/golem-push",
          value:
            update.oid === zero
              ? await git.resolveRef({ fs, ...repo, ref: "HEAD" })
              : update.oid,
        });
        await httpTransport(credentials, send, await redirectPolicy(repo)).run(
          async (http) => {
            if (!ref.startsWith("refs/")) {
              const advertised = await git.listServerRefs({
                http,
                url,
                forPush: true,
                protocolVersion: 1,
              });
              const candidates = new Set([
                ref,
                `refs/${ref}`,
                `refs/heads/${ref}`,
                `refs/tags/${ref}`,
              ]);
              const matches = [
                ...new Set(
                  advertised
                    .map((entry) => entry.ref)
                    .filter((entry) => candidates.has(entry)),
                ),
              ];
              if (matches.length > 1)
                throw new Error(
                  `destination refspec ${ref} matches more than one remote reference; qualify it`,
                );
              if (matches.length) ref = matches[0]!;
              else if (update.oid === zero)
                throw new Error(
                  `cannot delete missing remote reference: ${ref}`,
                );
              else
                ref = `${update.source?.startsWith("refs/tags/") ? "refs/tags/" : "refs/heads/"}${ref}`;
            }
            validRef(ref);
            if (ref.startsWith("refs/heads/") && update.oid !== zero)
              await git.readCommit({ fs, ...repo, oid: update.oid });
            let outcome: git.PushResult;
            try {
              outcome = await git.push({
                fs,
                ...staged,
                http,
                url,
                ref: "refs/heads/golem-push",
                remoteRef: ref,
                delete: update.oid === zero,
                force:
                  update.force ||
                  !!(options.forceWithLease && (!leaseRef || leaseRef === ref)),
                onPrePush: async ({ remoteRef }) => {
                  if (remoteRef.ref !== ref)
                    throw new Error(
                      `released library resolved a different destination: ${remoteRef.ref}; requested ${ref}; no push was sent`,
                    );
                  oldoid = remoteRef.oid;
                  if (update.oid === zero && oldoid === zero)
                    throw new Error(
                      `cannot delete missing remote reference: ${ref}`,
                    );
                  upToDate = oldoid === update.oid;
                  if (upToDate) return false;
                  if (
                    options.forceWithLease &&
                    !update.force &&
                    (!leaseRef || leaseRef === ref)
                  ) {
                    const target = trackingRefs(ref)[0];
                    const expected = leaseRef
                      ? leaseOid
                      : target
                        ? (localRefs.get(target) ?? zero)
                        : zero;
                    if (oldoid !== expected)
                      throw new Error(
                        `stale force-with-lease expectation for ${ref}`,
                      );
                  }
                  if (
                    ref.startsWith("refs/tags/") &&
                    oldoid !== zero &&
                    update.oid !== zero &&
                    !update.force &&
                    !(options.forceWithLease && (!leaseRef || leaseRef === ref))
                  )
                    throw new Error(
                      `existing remote tag requires force: ${ref}`,
                    );
                  return true;
                },
              });
            } catch (error) {
              if (
                !upToDate ||
                (error as { code?: string }).code !== "UserCanceledError"
              )
                throw error;
              return;
            }
            const status = outcome.refs[ref];
            if (
              !outcome.ok ||
              !status?.ok ||
              Object.keys(outcome.refs).length !== 1
            )
              throw new Error(
                status?.error ||
                  outcome.error ||
                  "invalid or incomplete receive-pack status; reconcile remote state before retrying",
              );
          },
        );
      } finally {
        await fs.promises.rm(gitdir, { recursive: true, force: true });
      }
      results.push({ url, ref, oid: update.oid, oldoid, ok: true, upToDate });
      const targets = trackingRefs(ref);
      if (targets.length)
        await publishRefs(repo, {
          writes:
            update.oid === zero
              ? []
              : targets.map((target) => [target, `${update.oid}\n`]),
          deletions: update.oid === zero ? targets : [],
        });
      if (
        options.setUpstream &&
        update.oid !== zero &&
        update.source?.startsWith("refs/heads/") &&
        ref.startsWith("refs/heads/")
      ) {
        await git.setConfig({
          fs,
          ...repo,
          path: `branch.${update.source.slice(11)}.remote`,
          value: remote,
        });
        await git.setConfig({
          fs,
          ...repo,
          path: `branch.${update.source.slice(11)}.merge`,
          value: ref,
        });
      }
    } catch (error) {
      failures.push(
        `${url} ${ref}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
  if (failures.length)
    throw new Error(
      [
        ...results.map(
          (result) =>
            `${result.url} ${result.ref}: ${result.upToDate ? "up-to-date" : "ok"}`,
        ),
        ...failures,
      ].join("\n"),
    );
  return { summary: `Pushed ${results.length} reference(s)`, refs: results };
}
