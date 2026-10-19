import fs from "node:fs";
import { Buffer } from "node:buffer";
import path from "node:path";
import * as git from "isomorphic-git";
import type { HttpClient } from "isomorphic-git";
import { GitConfigManager, GitRefManager } from "isomorphic-git/managers";
import { FileSystem } from "isomorphic-git/models";
import type { Repository } from "./git.ts";

const gitFs = new FileSystem(fs);

export type Credentials = {
  origin: string;
  username?: string;
  password?: string;
  bearer?: string;
  headers?: Record<string, string>;
};

export function httpUrl(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("remote must be an HTTP or HTTPS URL");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("SSH, git://, and filesystem transports are not provided");
  }
  if (url.username || url.password) {
    throw new Error(
      "URL credentials are not stored; pass an opaque authentication secret",
    );
  }
  if (url.hash) throw new Error("remote URLs must not contain fragments");
  return url;
}

export function redact(value: string, credentials?: Credentials): string {
  let result = value.replace(/(https?:\/\/)[^\s/@]+@/gi, "$1[redacted]@");
  for (const secret of [
    credentials?.password,
    credentials?.bearer,
    credentials?.password !== undefined
      ? Buffer.from(
          `${credentials.username ?? ""}:${credentials.password}`,
        ).toString("base64")
      : undefined,
    ...Object.values(credentials?.headers ?? {}),
  ]) {
    if (secret) result = result.split(secret).join("[redacted]");
  }
  return result;
}

function authHeaders(credentials: Credentials): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(credentials.headers ?? {})) {
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/i.test(name) || /[\r\n\0]/.test(value)) {
      throw new Error("invalid authentication header");
    }
    if (
      ["host", "content-length", "content-type", "transfer-encoding"].includes(
        name.toLowerCase(),
      )
    ) {
      throw new Error(
        "authentication headers must not override transport headers",
      );
    }
    headers[name.toLowerCase()] = value;
  }
  if (credentials.bearer !== undefined) {
    if (
      credentials.password !== undefined ||
      headers.authorization !== undefined
    ) {
      throw new Error("choose one authentication method");
    }
    if (/[\r\n\0]/.test(credentials.bearer))
      throw new Error("invalid bearer credential");
    headers.authorization = `Bearer ${credentials.bearer}`;
  } else if (credentials.password !== undefined) {
    if (headers.authorization !== undefined)
      throw new Error("choose one authentication method");
    if (credentials.username?.includes(":"))
      throw new Error("Basic authentication username must not contain ':'");
    headers.authorization = `Basic ${Buffer.from(`${credentials.username ?? ""}:${credentials.password}`).toString("base64")}`;
  }
  return headers;
}

/** The injected client exposes raw WASI HTTP responses, including redirects. */
export function httpTransport(
  credentials: Credentials | undefined,
  send: typeof fetch,
  followRedirects: "initial" | "true" | "false" = "initial",
): HttpClient & {
  run<T>(operation: (http: HttpClient) => Promise<T>): Promise<T>;
} {
  const credentialOrigin = credentials
    ? httpUrl(credentials.origin).origin
    : undefined;
  const secretHeaders = credentials ? authHeaders(credentials) : {};
  const canonicalBases = new Map<string, string>();
  const readers = new Set<ReadableStreamDefaultReader<Uint8Array>>();
  let failure: Error | undefined;
  function transportError(error: unknown): Error {
    return new Error(
      redact(
        error instanceof Error ? error.message : String(error),
        credentials,
      ),
    );
  }
  async function close(reader: ReadableStreamDefaultReader<Uint8Array>) {
    if (!readers.delete(reader)) return;
    try {
      await reader.cancel();
    } catch (error) {
      failure = transportError(error);
      throw failure;
    } finally {
      reader.releaseLock();
    }
  }
  const transport = {
    async run<T>(operation: (http: HttpClient) => Promise<T>): Promise<T> {
      try {
        const result = await operation(transport);
        if (failure) throw failure;
        return result;
      } finally {
        await Promise.all([...readers].map(close));
      }
    },
    async request(request: Parameters<HttpClient["request"]>[0]) {
      let original = httpUrl(request.url);
      for (const [from, to] of canonicalBases) {
        if (original.href.startsWith(`${from}/`)) {
          original = httpUrl(to + original.href.slice(from.length));
          break;
        }
      }
      let url = original;
      const method = request.method ?? "GET";
      const chunks: Uint8Array[] = [];
      if (request.body)
        for await (const chunk of request.body) chunks.push(chunk);
      const body = request.body ? Buffer.concat(chunks) : undefined;
      const headers = { ...request.headers };
      let credentialAllowed = original.origin === credentialOrigin;
      try {
        for (let redirects = 0; ; redirects += 1) {
          const response = await send(url.href, {
            method,
            headers: {
              ...headers,
              ...(credentialAllowed ? secretHeaders : {}),
            },
            body,
            redirect: "manual",
            signal: request.signal,
          });
          const reader = response.body?.getReader();
          if (reader) readers.add(reader);
          if ([301, 302, 303, 307, 308].includes(response.status)) {
            const location = response.headers.get("location");
            if (reader) await close(reader);
            if (
              followRedirects === "false" ||
              (followRedirects === "initial" && method !== "GET")
            ) {
              throw new Error(
                "remote redirect is disabled by http.followRedirects",
              );
            }
            if (!location) throw new Error("remote redirect has no Location");
            if (redirects >= 20) throw new Error("too many remote redirects");
            const next = httpUrl(new URL(location, url).href);
            if (next.origin !== url.origin) {
              credentialAllowed = false;
              for (const name of Object.keys(headers)) {
                if (
                  ["authorization", "proxy-authorization", "cookie"].includes(
                    name.toLowerCase(),
                  )
                )
                  delete headers[name];
              }
            }
            url = next;
            continue;
          }
          if (response.status === 401 || response.status === 403) {
            if (reader) await close(reader);
            throw new Error(
              `remote authentication failed (HTTP ${response.status}); noninteractive credentials are missing, expired, or denied`,
            );
          }
          if (
            method === "GET" &&
            original.pathname.endsWith("/info/refs") &&
            url.href !== original.href
          ) {
            if (
              !url.pathname.endsWith("/info/refs") ||
              url.search !== original.search
            ) {
              if (reader) await close(reader);
              throw new Error(
                "remote discovery redirect does not identify a Git repository base",
              );
            }
            canonicalBases.set(
              original.href.slice(0, original.href.indexOf("/info/refs")),
              url.href.slice(0, url.href.indexOf("/info/refs")),
            );
          }
          return {
            url: url.href,
            method,
            statusCode: response.status,
            statusMessage: response.statusText,
            headers: Object.fromEntries(response.headers.entries()),
            body: (async function* () {
              if (!reader) return;
              try {
                for (;;) {
                  const { done, value } = await reader.read();
                  if (done) return;
                  yield value;
                }
              } catch (error) {
                failure = transportError(error);
                throw failure;
              } finally {
                await close(reader);
              }
            })(),
          };
        }
      } catch (error) {
        throw new Error(
          redact(
            error instanceof Error ? error.message : String(error),
            credentials,
          ),
        );
      }
    },
  };
  return transport;
}

export function configKey(key: string): void {
  if (
    !/^(user\.(name|email)|remote\..+\.(url|pushurl|fetch|push|tagopt|prune)|branch\..+\.(remote|merge|pushremote|rebase)|remote\.pushdefault|push\.default|fetch\.(prune|prunetags)|pull\.(ff|rebase)|merge\.ff|http\.followredirects)$/i.test(
      key,
    ) ||
    /[\0\r\n"\\]/.test(key)
  ) {
    throw new Error(`unsupported repository-local configuration key: ${key}`);
  }
}

export function configValue(key: string, value: string): void {
  configKey(key);
  if (
    value.trim() !== value ||
    /[\0\r\n"\u2028\u2029]/.test(value) ||
    (/[#;]/.test(value) && value.endsWith("\\"))
  ) {
    throw new Error(
      "configuration value cannot be represented safely in the local Git config",
    );
  }
  if (/^remote\..+\.(url|pushurl)$/i.test(key)) httpUrl(value);
  if (
    /^http\.followredirects$/i.test(key) &&
    !["true", "false", "initial"].includes(value)
  ) {
    throw new Error("http.followRedirects must be true, false, or initial");
  }
}

export async function configure(
  repo: Repository,
  key: string,
  value: string | undefined,
  options: { add: boolean; getAll: boolean; unset: boolean; unsetAll: boolean },
) {
  configKey(key);
  if (
    [options.add, options.getAll, options.unset, options.unsetAll].filter(
      Boolean,
    ).length > 1
  ) {
    throw new Error("configuration actions are mutually exclusive");
  }
  if (
    (options.unset || options.unsetAll || options.getAll) &&
    value !== undefined
  ) {
    throw new Error("this configuration action does not accept a value");
  }
  if (options.add && value === undefined)
    throw new Error("--add requires a value");
  const config = await GitConfigManager.get({ fs: gitFs, gitdir: repo.gitdir });
  const existing: string[] = await config.getall(key);
  if (
    !options.add &&
    (options.unset || (value !== undefined && !options.unsetAll)) &&
    existing.length > 1
  ) {
    throw new Error(
      "configuration key has multiple values; use --get-all or --unset-all",
    );
  }
  const updated = value !== undefined || options.unset || options.unsetAll;
  if (value !== undefined) {
    configValue(key, value);
    await config.set(key, value, options.add);
  } else if (options.unset || options.unsetAll) {
    if (!existing.length)
      throw new Error(`local configuration is not set: ${key}`);
    for (let index = 0; index < existing.length; index += 1)
      await config.set(key, undefined);
  }
  if (updated)
    await GitConfigManager.save({ fs: gitFs, gitdir: repo.gitdir, config });
  const values: string[] = await config.getall(key);
  if (!updated && !values.length)
    throw new Error(`local configuration is not set: ${key}`);
  return {
    key,
    value: values[values.length - 1],
    values: options.getAll ? values : undefined,
    updated,
  };
}

export async function remoteNames(repo: Repository): Promise<string[]> {
  const config = await GitConfigManager.get({ fs: gitFs, gitdir: repo.gitdir });
  const sections: unknown[] = await config.getSubsections("remote");
  return [
    ...new Set(
      sections.filter((name): name is string => typeof name === "string"),
    ),
  ].sort();
}

async function requireRemote(repo: Repository, remote: string): Promise<void> {
  if (!(await remoteNames(repo)).includes(remote))
    throw new Error(`no such remote: ${remote}`);
}

function remoteName(name: string): void {
  if (
    !name ||
    /[\x00-\x20\x7f"\\:*?\[~^]/.test(name) ||
    name.startsWith("-") ||
    name.includes("..") ||
    name.includes("@{") ||
    name.startsWith("/") ||
    name.endsWith("/") ||
    name
      .split("/")
      .some(
        (part) =>
          !part ||
          part.startsWith(".") ||
          part.endsWith(".") ||
          part.endsWith(".lock"),
      )
  ) {
    throw new Error("invalid remote name");
  }
}

export async function remoteUrls(
  repo: Repository,
  name: string,
  push = false,
): Promise<string[]> {
  await requireRemote(repo, name);
  const pushUrls = push
    ? await git.getConfigAll({ fs, ...repo, path: `remote.${name}.pushurl` })
    : [];
  return pushUrls.length
    ? pushUrls
    : git.getConfigAll({ fs, ...repo, path: `remote.${name}.url` });
}

export async function remoteList(
  repo: Repository,
  verbose: boolean,
): Promise<string> {
  const lines: string[] = [];
  for (const name of await remoteNames(repo)) {
    if (!verbose) lines.push(name);
    else {
      for (const url of await remoteUrls(repo, name))
        lines.push(`${name}\t${redact(url)} (fetch)`);
      for (const url of await remoteUrls(repo, name, true))
        lines.push(`${name}\t${redact(url)} (push)`);
    }
  }
  return lines.join("\n") + (lines.length ? "\n" : "");
}

export async function remoteAdd(
  repo: Repository,
  name: string,
  url: string,
): Promise<void> {
  remoteName(name);
  configValue(`remote.${name}.url`, url);
  if ((await remoteNames(repo)).includes(name))
    throw new Error(`remote already exists: ${name}`);
  await git.addRemote({ fs, ...repo, remote: name, url });
}

export async function remoteSetUrl(
  repo: Repository,
  name: string,
  url: string,
  push: boolean,
  add: boolean,
  remove: boolean,
  old?: string,
): Promise<void> {
  await requireRemote(repo, name);
  const key = `remote.${name}.${push ? "pushurl" : "url"}`;
  if (add && remove)
    throw new Error("--add and --delete are mutually exclusive");
  if (!remove) configValue(key, url);
  const existing = await git.getConfigAll({ fs, ...repo, path: key });
  if (add) {
    await git.setConfig({ fs, ...repo, path: key, value: url, append: true });
    return;
  }
  const pattern = remove ? url : old;
  const regex = pattern === undefined ? undefined : new RegExp(pattern);
  const selected = existing.findIndex((entry) => !regex || regex.test(entry));
  if (regex && selected < 0)
    throw new Error("no remote URL matched the pattern");
  const updated = remove
    ? existing.filter((entry) => !regex!.test(entry))
    : existing.length
      ? existing.map((entry, index) => (index === selected ? url : entry))
      : [url];
  if (!push && !updated.length)
    throw new Error("cannot delete every fetch URL");
  for (let index = 0; index < existing.length; index += 1)
    await git.setConfig({ fs, ...repo, path: key, value: undefined });
  for (const entry of updated)
    await git.setConfig({ fs, ...repo, path: key, value: entry, append: true });
}

export async function remoteRenameOrRemove(
  repo: Repository,
  name: string,
  replacement?: string,
): Promise<void> {
  await requireRemote(repo, name);
  if (replacement !== undefined) {
    remoteName(replacement);
    if ((await remoteNames(repo)).includes(replacement))
      throw new Error(`remote already exists: ${replacement}`);
  }
  const config = await GitConfigManager.get({ fs: gitFs, gitdir: repo.gitdir });
  const keys = ["url", "pushurl", "fetch", "push", "tagopt", "prune"];
  const fetchSpecs: string[] = await config.getall(`remote.${name}.fetch`);
  const otherSpecs: string[] = [];
  for (const other of await remoteNames(repo)) {
    if (other !== name)
      otherSpecs.push(...(await config.getall(`remote.${other}.fetch`)));
  }
  if (replacement) {
    for (const key of keys) {
      for (const value of await config.getall(`remote.${name}.${key}`)) {
        await config.append(
          `remote.${replacement}.${key}`,
          key === "fetch"
            ? String(value).replace(
                `:refs/remotes/${name}/`,
                `:refs/remotes/${replacement}/`,
              )
            : value,
        );
      }
    }
  }
  for (const branch of await config.getSubsections("branch")) {
    if ((await config.get(`branch.${branch}.remote`)) === name) {
      for (const _ of await config.getall(`branch.${branch}.remote`))
        await config.set(`branch.${branch}.remote`, undefined);
      await config.set(`branch.${branch}.remote`, replacement);
      if (!replacement) {
        for (const _ of await config.getall(`branch.${branch}.merge`))
          await config.set(`branch.${branch}.merge`, undefined);
      }
    }
    if ((await config.get(`branch.${branch}.pushRemote`)) === name)
      await config.set(`branch.${branch}.pushRemote`, replacement);
  }
  if ((await config.get("remote.pushDefault")) === name)
    await config.set("remote.pushDefault", replacement);
  const allRefs = (
    await GitRefManager.listRefs({
      fs: gitFs,
      gitdir: repo.gitdir,
      filepath: "refs",
    })
  ).map((ref) => `refs/${ref}`);
  const refs = allRefs.filter((ref) =>
    replacement
      ? ref.startsWith(`refs/remotes/${name}/`)
      : !ref.startsWith("refs/heads/") &&
        (ref.startsWith(`refs/remotes/${name}/`) ||
          reverseRefspecs(fetchSpecs, ref) !== undefined) &&
        reverseRefspecs(otherSpecs, ref) === undefined,
  );
  if (replacement) {
    for (const ref of refs) {
      if (
        allRefs.includes(
          ref.replace(`refs/remotes/${name}/`, `refs/remotes/${replacement}/`),
        )
      )
        throw new Error(
          "remote rename would overwrite an existing tracking reference",
        );
    }
  }
  for (const ref of refs) {
    if (replacement) {
      const value = await GitRefManager.resolve({
        fs: gitFs,
        gitdir: repo.gitdir,
        ref,
        depth: 1,
      });
      const target = ref.replace(
        `refs/remotes/${name}/`,
        `refs/remotes/${replacement}/`,
      );
      if (value.startsWith("ref: "))
        await git.writeRef({
          fs,
          ...repo,
          ref: target,
          value: value
            .slice(5)
            .replace(`refs/remotes/${name}/`, `refs/remotes/${replacement}/`),
          symbolic: true,
        });
      else await git.writeRef({ fs, ...repo, ref: target, value });
      const log = path.join(repo.gitdir, "logs", ref);
      const targetLog = path.join(repo.gitdir, "logs", target);
      await fs.promises.mkdir(path.dirname(targetLog), { recursive: true });
      await fs.promises.rename(log, targetLog).catch((error) => {
        if (error.code !== "ENOENT") throw error;
      });
    }
    await git.deleteRef({ fs, ...repo, ref });
    await fs.promises.rm(path.join(repo.gitdir, "logs", ref), { force: true });
  }
  await config.deleteSection("remote", name);
  await GitConfigManager.save({ fs: gitFs, gitdir: repo.gitdir, config });
}

export async function lsRemote(
  repo: Repository | undefined,
  remote: string | undefined,
  heads: boolean,
  tags: boolean,
  symref: boolean,
  send: typeof fetch,
  credentials?: Credentials,
) {
  let url = remote;
  if (
    repo &&
    (remote === undefined || (await remoteNames(repo)).includes(remote))
  ) {
    const branch = await git.currentBranch({ fs, ...repo });
    const name =
      remote ??
      (branch
        ? await git.getConfig({ fs, ...repo, path: `branch.${branch}.remote` })
        : undefined) ??
      "origin";
    url = (await remoteUrls(repo, name))[0];
  }
  if (!url) throw new Error("no remote repository specified");
  httpUrl(url);
  const refs = await httpTransport(
    credentials,
    send,
    repo ? await redirectPolicy(repo) : "initial",
  ).run((http) =>
    git.listServerRefs({ http, url, symrefs: true, peelTags: true }),
  );
  const selected = refs.filter(
    ({ ref }) =>
      (!heads && !tags) ||
      (heads && ref.startsWith("refs/heads/")) ||
      (tags && ref.startsWith("refs/tags/")),
  );
  const lines: string[] = [];
  for (const { ref, oid, target, peeled } of selected) {
    if (symref && target) lines.push(`ref: ${target}\t${ref}`);
    lines.push(`${oid}\t${ref}`);
    if (peeled) lines.push(`${peeled}\t${ref}^{}`);
  }
  return {
    refs: selected,
    stdout: lines.join("\n") + (lines.length ? "\n" : ""),
  };
}

export async function redirectPolicy(
  repo: Repository,
): Promise<"initial" | "true" | "false"> {
  const value =
    (await git.getConfig({ fs, ...repo, path: "http.followRedirects" })) ??
    "initial";
  if (value !== "true" && value !== "false" && value !== "initial")
    throw new Error("invalid http.followRedirects setting");
  return value;
}

export function refspecDestination(
  spec: string,
  source: string,
): string | undefined {
  const positive = spec.startsWith("+") ? spec.slice(1) : spec;
  if (positive.startsWith("^")) return undefined;
  const parts = positive.split(":");
  if (parts.length > 2) throw new Error(`invalid refspec: ${spec}`);
  const [from, to] = parts;
  if (!from || !to) return undefined;
  const star = from.indexOf("*");
  if (star < 0) return from === source ? to : undefined;
  if (
    from.indexOf("*", star + 1) >= 0 ||
    !to.includes("*") ||
    to.indexOf("*", to.indexOf("*") + 1) >= 0
  )
    throw new Error(`invalid wildcard refspec: ${spec}`);
  const prefix = from.slice(0, star);
  const suffix = from.slice(star + 1);
  if (
    !source.startsWith(prefix) ||
    !source.endsWith(suffix) ||
    source.length < prefix.length + suffix.length
  )
    return undefined;
  return to.replace(
    "*",
    source.slice(prefix.length, source.length - suffix.length),
  );
}

function excluded(specs: string[], source: string): boolean {
  return specs.some(
    (spec) =>
      spec.startsWith("^") &&
      refspecDestination(`${spec.slice(1)}:${spec.slice(1)}`, source) !==
        undefined,
  );
}

function reverseRefspecs(
  specs: string[],
  destination: string,
): string | undefined {
  for (const spec of specs) {
    if (spec.startsWith("^")) continue;
    const [source, target] = spec.replace(/^\+/, "").split(":");
    if (!source || !target) continue;
    const mapped = refspecDestination(`${target}:${source}`, destination);
    if (mapped && !excluded(specs, mapped)) return mapped;
  }
  return undefined;
}

export async function upstream(repo: Repository, branch: string) {
  const remote = await git.getConfig({
    fs,
    ...repo,
    path: `branch.${branch}.remote`,
  });
  const merge = await git.getConfigAll({
    fs,
    ...repo,
    path: `branch.${branch}.merge`,
  });
  if (!remote || !merge.length) return undefined;
  if (remote === ".") return { remote, merge: merge[0]!, ref: merge[0]! };
  const specs = await git.getConfigAll({
    fs,
    ...repo,
    path: `remote.${remote}.fetch`,
  });
  if (excluded(specs, merge[0]!))
    throw new Error("upstream is excluded by a negative fetch refspec");
  for (const spec of specs) {
    const ref = refspecDestination(spec, merge[0]!);
    if (ref) return { remote, merge: merge[0]!, ref };
  }
  throw new Error(
    `upstream is not mapped by remote ${remote}'s fetch refspecs`,
  );
}

export type Tracking = { ref: string; remote: string; merge: string };

export async function trackingForRef(
  repo: Repository,
  target: string,
): Promise<Tracking | undefined> {
  const ref = await git.expandRef({ fs, ...repo, ref: target });
  if (ref.startsWith("refs/heads/")) return { ref, remote: ".", merge: ref };
  let selected: { remote: string; merge: string } | undefined;
  for (const remote of await remoteNames(repo)) {
    const merge = reverseRefspecs(
      await git.getConfigAll({ fs, ...repo, path: `remote.${remote}.fetch` }),
      ref,
    );
    if (merge) {
      if (selected && (selected.remote !== remote || selected.merge !== merge))
        throw new Error("upstream reference is mapped by multiple remotes");
      selected = { remote, merge };
    }
  }
  return selected ? { ref, ...selected } : undefined;
}

export async function setUpstream(
  repo: Repository,
  branch: string,
  target: string | Tracking | undefined,
) {
  if (!(await git.listBranches({ fs, ...repo })).includes(branch))
    throw new Error(`branch does not exist: ${branch}`);
  const config = await GitConfigManager.get({ fs: gitFs, gitdir: repo.gitdir });
  const selected =
    typeof target === "string" ? await trackingForRef(repo, target) : target;
  if (target !== undefined) {
    if (!selected)
      throw new Error(
        "upstream must be a local or mapped remote-tracking branch",
      );
  } else if (!(await config.get(`branch.${branch}.remote`)))
    throw new Error(`branch has no upstream: ${branch}`);
  for (const key of ["remote", "merge"]) {
    for (const _ of await config.getall(`branch.${branch}.${key}`))
      await config.set(`branch.${branch}.${key}`, undefined);
  }
  await config.set(`branch.${branch}.remote`, selected?.remote);
  await config.set(`branch.${branch}.merge`, selected?.merge);
  await GitConfigManager.save({ fs: gitFs, gitdir: repo.gitdir, config });
}

export async function trackingCheckoutTarget(
  repo: Repository,
  name: string,
): Promise<Tracking | undefined> {
  try {
    await git.resolveRef({ fs, ...repo, ref: name });
    return undefined;
  } catch (error) {
    if ((error as { code?: string }).code !== "NotFoundError") throw error;
  }
  const matches: Tracking[] = [];
  for (const remote of await remoteNames(repo)) {
    const specs = await git.getConfigAll({
      fs,
      ...repo,
      path: `remote.${remote}.fetch`,
    });
    for (const ref of (
      await GitRefManager.listRefs({
        fs: gitFs,
        gitdir: repo.gitdir,
        filepath: "refs",
      })
    ).map((ref) => `refs/${ref}`)) {
      const merge = reverseRefspecs(specs, ref);
      if (merge === `refs/heads/${name}`) matches.push({ ref, remote, merge });
    }
  }
  if (matches.length > 1)
    throw new Error(
      "branch name matches multiple remote-tracking branches; specify the remote explicitly",
    );
  return matches[0];
}

export async function trackedBranches(
  repo: Repository,
  remoteOnly: boolean,
  all: boolean,
  verbose: number,
) {
  const current = await git.currentBranch({ fs, ...repo });
  const refs = [
    ...(!remoteOnly
      ? (await git.listBranches({ fs, ...repo })).map(
          (name) => `refs/heads/${name}`,
        )
      : []),
    ...(remoteOnly || all
      ? (
          await GitRefManager.listRefs({
            fs: gitFs,
            gitdir: repo.gitdir,
            filepath: "refs/remotes",
          })
        ).map((name) => `refs/remotes/${name}`)
      : []),
  ];
  return Promise.all(
    refs.sort().map(async (ref) => {
      const remote = ref.startsWith("refs/remotes/");
      const name = ref.replace(
        remote ? /^refs\/remotes\// : /^refs\/heads\//,
        "",
      );
      const oid = await git.resolveRef({ fs, ...repo, ref });
      const tracking =
        !remote && verbose > 1 ? await upstream(repo, name) : undefined;
      let gone = false;
      let ahead: number | undefined;
      let behind: number | undefined;
      if (tracking) {
        let trackingOid: string | undefined;
        try {
          trackingOid = await git.resolveRef({
            fs,
            ...repo,
            ref: tracking.ref,
          });
        } catch (error) {
          if ((error as { code?: string }).code !== "NotFoundError")
            throw error;
          gone = true;
        }
        if (trackingOid) {
          const local = new Set(
            (await git.log({ fs, ...repo, ref: oid })).map(({ oid }) => oid),
          );
          const upstreamCommits = new Set(
            (await git.log({ fs, ...repo, ref: trackingOid })).map(
              ({ oid }) => oid,
            ),
          );
          ahead = [...local].filter((oid) => !upstreamCommits.has(oid)).length;
          behind = [...upstreamCommits].filter((oid) => !local.has(oid)).length;
        }
      }
      return {
        name,
        current: !remote && name === current,
        oid,
        remote,
        upstream: tracking?.ref,
        gone: tracking ? gone : undefined,
        ahead,
        behind,
      };
    }),
  );
}
