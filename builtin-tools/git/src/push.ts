import fs from "node:fs";
import * as git from "../golem-temp/isomorphic-git/index.js";
import { GitRefManager } from "../golem-temp/isomorphic-git/managers/index.js";
import { FileSystem } from "../golem-temp/isomorphic-git/models/index.js";
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
import { publishRefs, validRef } from "./transfer.ts";

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
  const localRefs = new Map<string, string>();
  for (const ref of await GitRefManager.listRefs({
    fs: new FileSystem(fs),
    gitdir: repo.gitdir,
    filepath: "refs",
  })) {
    const name = `refs/${ref}`;
    try {
      localRefs.set(name, await git.resolveRef({ fs, ...repo, ref: name }));
    } catch (error) {
      const raw = await GitRefManager.resolve({
        fs: new FileSystem(fs),
        gitdir: repo.gitdir,
        ref: name,
        depth: 1,
      });
      if (
        (error as { code?: string }).code !== "NotFoundError" ||
        !raw.startsWith("ref: ")
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
    if (mode === "matching") specs = [":"];
    else {
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
  }
  if (specs.some((spec) => spec.startsWith("^")))
    throw new Error("negative push refspecs are not supported by Git");
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
    const selected = new Map<string, Update>();
    const unconditionalForce = new Set<string>();
    try {
      const outcome = await httpTransport(
        credentials,
        send,
        await redirectPolicy(repo),
      ).run((http) =>
        git.pushRefs({
          fs,
          ...repo,
          http,
          url,
          atomic: options.atomic,
          plan: async (advertised) => {
            const add = (update: Update) => {
              validRef(update.ref);
              if (update.force) unconditionalForce.add(update.ref);
              if (
                options.forceWithLease &&
                (!leaseRef || leaseRef === update.ref)
              )
                update.force = true;
              const prior = selected.get(update.ref);
              if (
                prior &&
                (prior.oid !== update.oid || prior.source !== update.source)
              )
                throw new Error(`conflicting push destination: ${update.ref}`);
              selected.set(update.ref, update);
            };
            for (const raw of specs) {
              const spec = raw.replace(/^\+/, "");
              const force = !!(options.force || raw.startsWith("+"));
              if (spec === ":") {
                for (const [ref, oid] of localRefs)
                  if (ref.startsWith("refs/heads/") && advertised.has(ref))
                    add({ source: ref, ref, oid, force });
                continue;
              }
              const [source, destination, extra] = spec.split(":");
              if (
                extra !== undefined ||
                source === undefined ||
                destination === ""
              )
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
              let local: string | undefined;
              let oid = zero;
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
                const selectedRefs = strong.length ? strong : matches;
                if (selectedRefs.length > 1)
                  throw new Error(
                    `src refspec ${source} matches more than one`,
                  );
                const expanded: string =
                  selectedRefs[0] ??
                  (source === "HEAD" && branch
                    ? `refs/heads/${branch}`
                    : /^[0-9a-f]{40}$/.test(source)
                      ? source
                      : await git.expandRef({ fs, ...repo, ref: source }));
                local = expanded;
                oid = await git.resolveRef({ fs, ...repo, ref: expanded });
              }
              let target = options.delete ? source : destination || local;
              if (!target) throw new Error("deletion requires a destination");
              if (!target.startsWith("refs/")) {
                const candidates = [...advertised.keys()].filter(
                  (ref) =>
                    ref === `refs/heads/${target}` ||
                    ref === `refs/tags/${target}`,
                );
                if (candidates.length > 1)
                  throw new Error(`ambiguous push destination: ${target}`);
                target =
                  candidates[0] ??
                  `${local?.startsWith("refs/tags/") ? "refs/tags" : "refs/heads"}/${target}`;
                if (
                  !candidates.length &&
                  !deleting &&
                  !local?.startsWith("refs/heads/") &&
                  !local?.startsWith("refs/tags/")
                )
                  throw new Error(
                    "push of an object ID or detached HEAD requires a fully qualified destination",
                  );
              }
              add({ source: local, ref: target, oid, force });
            }
            return [...selected.values()];
          },
          onPrePush: async (ref, oldoid) => {
            if (
              !options.forceWithLease ||
              unconditionalForce.has(ref) ||
              (leaseRef && leaseRef !== ref)
            )
              return;
            let expected = leaseOid;
            if (!leaseRef) {
              const target = trackingRefs(ref)[0];
              expected = target ? (localRefs.get(target) ?? zero) : zero;
            }
            if (oldoid !== expected)
              throw new Error(`stale force-with-lease expectation for ${ref}`);
          },
        }),
      );
      const writes: [string, string][] = [],
        deletions: string[] = [];
      for (const result of outcome) {
        results.push({ url, ...result });
        if (!result.ok) continue;
        for (const target of trackingRefs(result.ref)) {
          if (result.oid === zero) deletions.push(target);
          else writes.push([target, `${result.oid}\n`]);
        }
      }
      if (writes.length || deletions.length)
        await publishRefs(repo, { writes, deletions });
      if (options.setUpstream)
        for (const result of outcome) {
          const source = selected.get(result.ref)?.source;
          if (
            !result.ok ||
            result.oid === zero ||
            !source?.startsWith("refs/heads/") ||
            !result.ref.startsWith("refs/heads/")
          )
            continue;
          await git.setConfig({
            fs,
            ...repo,
            path: `branch.${source.slice(11)}.remote`,
            value: remote,
          });
          await git.setConfig({
            fs,
            ...repo,
            path: `branch.${source.slice(11)}.merge`,
            value: result.ref,
          });
        }
    } catch (error) {
      failures.push(
        `${url}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
  const rejected = results.filter((result) => !result.ok);
  if (failures.length || rejected.length)
    throw new Error(
      [
        ...results.map(
          (result) =>
            `${result.url} ${result.ref}: ${result.ok ? (result.upToDate ? "up-to-date" : "ok") : (result.error ?? "rejected")}`,
        ),
        ...failures,
      ].join("\n"),
    );
  return {
    summary: results.length
      ? `Pushed ${results.length} references`
      : "Everything up-to-date",
    refs: results,
  };
}
