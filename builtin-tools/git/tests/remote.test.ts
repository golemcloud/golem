import assert from "node:assert/strict";
import fs from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import * as git from "isomorphic-git";
import { localConfig, repository } from "../src/git.ts";
import {
  configure,
  httpTransport,
  httpUrl,
  lsRemote,
  redact,
  remoteAdd,
  remoteList,
  remoteRenameOrRemove,
  remoteSetUrl,
  remoteUrls,
  setUpstream,
  trackedBranches,
  trackingCheckoutTarget,
  upstream,
} from "../src/remote.ts";

async function fixture(t: test.TestContext) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "golem-git-remote-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await git.init({ fs, dir, defaultBranch: "main" });
  return repository([dir]);
}

test("remote URLs are HTTP(S) and credentials never enter repository config", async (t) => {
  const repo = await fixture(t);
  for (const url of [
    "ssh://example.org/repo",
    "git://example.org/repo",
    "https://alice:password@example.org/repo",
  ]) {
    await assert.rejects(remoteAdd(repo, "origin", url));
  }
  await remoteAdd(repo, "origin", "https://example.org/repo");
  assert.equal(await remoteList(repo, false), "origin\n");
  await assert.rejects(
    remoteAdd(repo, "origin", "https://other.org/repo"),
    /already exists/,
  );
  assert.equal(
    await localConfig(repo, "remote.origin.fetch", undefined),
    "+refs/heads/*:refs/remotes/origin/*",
  );
  assert.ok(
    !(await readFile(path.join(repo.gitdir, "config"), "utf8")).includes(
      "password",
    ),
  );
});

test("multiple fetch and push URLs have independent add, replace and delete semantics", async (t) => {
  const repo = await fixture(t);
  await remoteAdd(repo, "origin", "https://example.org/fetch");
  await remoteSetUrl(
    repo,
    "origin",
    "https://example.org/push",
    true,
    true,
    false,
  );
  await remoteSetUrl(
    repo,
    "origin",
    "https://mirror.org/push",
    true,
    true,
    false,
  );
  assert.deepEqual(await remoteUrls(repo, "origin"), [
    "https://example.org/fetch",
  ]);
  assert.deepEqual(await remoteUrls(repo, "origin", true), [
    "https://example.org/push",
    "https://mirror.org/push",
  ]);
  await remoteSetUrl(
    repo,
    "origin",
    "https://new.org/push",
    true,
    false,
    false,
    "mirror\\.org",
  );
  await remoteSetUrl(repo, "origin", "example\\.org", true, false, true);
  assert.deepEqual(await remoteUrls(repo, "origin", true), [
    "https://new.org/push",
  ]);
  await assert.rejects(
    remoteSetUrl(repo, "origin", ".*", false, false, true),
    /every fetch URL/,
  );
});

test("remote rename preserves symbolic HEAD and updates tracking and push configuration", async (t) => {
  const repo = await fixture(t);
  const oid = "1234567890123456789012345678901234567890";
  await remoteAdd(repo, "old", "https://example.org/repo");
  await localConfig(repo, "branch.main.remote", "old");
  await localConfig(repo, "branch.main.merge", "refs/heads/main");
  await localConfig(repo, "branch.main.pushRemote", "old");
  await localConfig(repo, "remote.pushDefault", "old");
  await git.writeRef({ fs, ...repo, ref: "refs/remotes/old/main", value: oid });
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/remotes/old/HEAD",
    value: "refs/remotes/old/main",
    symbolic: true,
  });
  await remoteRenameOrRemove(repo, "old", "new");
  assert.equal(await remoteList(repo, false), "new\n");
  assert.equal(await localConfig(repo, "branch.main.remote", undefined), "new");
  assert.equal(
    await localConfig(repo, "branch.main.pushRemote", undefined),
    "new",
  );
  assert.equal(await localConfig(repo, "remote.pushDefault", undefined), "new");
  assert.equal(
    await localConfig(repo, "remote.new.fetch", undefined),
    "+refs/heads/*:refs/remotes/new/*",
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/new/HEAD" }),
    oid,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "refs/remotes/new/HEAD"), "utf8"),
    "ref: refs/remotes/new/main\n",
  );
  await remoteRenameOrRemove(repo, "new");
  assert.equal(await remoteList(repo, false), "");
  assert.deepEqual(await git.listBranches({ fs, ...repo, remote: "new" }), []);
  assert.equal(
    await git.getConfig({ fs, ...repo, path: "branch.main.merge" }),
    undefined,
  );
});

test("HTTP credentials are isolated across origins and discovery redirect carries into POST", async (t) => {
  for (const target of ["https://target.org", "http://source.org"]) {
    await t.test(target, async () => {
      const requests: { url: string; headers: Headers; method: string }[] = [];
      const transport = httpTransport(
        { origin: "https://source.org", bearer: "sensitive" },
        async (input, options) => {
          const url = String(input);
          requests.push({
            url,
            headers: new Headers(options?.headers),
            method: options?.method ?? "GET",
          });
          return url.startsWith("https://source.org")
            ? new Response(null, {
                status: 302,
                headers: {
                  location: `${target}/new/info/refs?service=git-upload-pack`,
                },
              })
            : new Response("reply");
        },
      );
      const discovery = await transport.request({
        url: "https://source.org/repo/info/refs?service=git-upload-pack",
      });
      for await (const _ of discovery.body!) {
        /* consume */
      }
      const transfer = await transport.request({
        url: "https://source.org/repo/git-upload-pack",
        method: "POST",
      });
      for await (const _ of transfer.body!) {
        /* consume */
      }
      assert.equal(
        requests[0]?.headers.get("authorization"),
        "Bearer sensitive",
      );
      assert.equal(requests[1]?.headers.get("authorization"), null);
      assert.equal(requests[2]?.headers.get("authorization"), null);
      assert.equal(requests[2]?.url, `${target}/new/git-upload-pack`);
      assert.equal(requests[2]?.method, "POST");
    });
  }
});

test("same-origin redirects keep credentials and preserve POST for every Git redirect status", async (t) => {
  for (const status of [301, 302, 303, 307, 308]) {
    await t.test(String(status), async () => {
      const seen: {
        body: Uint8Array | undefined;
        authorization: string | null;
        method: string | undefined;
      }[] = [];
      const transport = httpTransport(
        { origin: "https://example.org", username: "alice", password: "token" },
        async (_url, options) => {
          seen.push({
            body: options?.body as Uint8Array | undefined,
            authorization: new Headers(options?.headers).get("authorization"),
            method: options?.method,
          });
          return seen.length === 1
            ? new Response(null, {
                status,
                headers: { location: "/new/git-receive-pack" },
              })
            : new Response("ok");
        },
        "true",
      );
      const bytes = new Uint8Array([0, 255, 17]);
      const response = await transport.request({
        url: "https://example.org/repo/git-receive-pack",
        method: "POST",
        body: (async function* () {
          yield bytes;
        })(),
      });
      for await (const _ of response.body!) {
        /* consume */
      }
      assert.deepEqual([...seen[1]!.body!], [...bytes]);
      assert.equal(seen[1]!.method, "POST");
      assert.equal(
        seen[1]!.authorization,
        `Basic ${Buffer.from("alice:token").toString("base64")}`,
      );
    });
  }
});

test("expired credentials, redirect loops and unsafe POST redirects fail without secrets", async () => {
  for (const status of [401, 403]) {
    const transport = httpTransport(
      { origin: "https://example.org", bearer: "token" },
      async () => new Response("denied", { status }),
    );
    await assert.rejects(
      transport.request({ url: "https://example.org/repo" }),
      /noninteractive credentials/,
    );
  }
  const loop = httpTransport(
    undefined,
    async () =>
      new Response(null, { status: 302, headers: { location: "/repo" } }),
  );
  await assert.rejects(
    loop.request({ url: "https://example.org/repo" }),
    /too many remote redirects/,
  );
  await assert.rejects(
    loop.request({ url: "https://example.org/repo", method: "POST" }),
    /followRedirects/,
  );
  const failure = httpTransport(
    { origin: "https://example.org", bearer: "secret" },
    async () => {
      throw new Error("network error secret");
    },
  );
  await assert.rejects(
    failure.request({ url: "https://example.org/repo" }),
    (error: Error) => !error.message.includes("secret"),
  );
  assert.equal(
    redact("https://alice:secret@example.org/repo"),
    "https://[redacted]@example.org/repo",
  );
  assert.throws(
    () => httpUrl("https://user:secret@example.org/repo"),
    /opaque/,
  );
});

test("discovery rejects a transport failure instead of returning partial advertised refs", async () => {
  const pkt = (value: string) =>
    `${(value.length + 4).toString(16).padStart(4, "0")}${value}`;
  const prefix =
    pkt("# service=git-upload-pack\n") +
    "0000" +
    pkt(
      "1234567890123456789012345678901234567890 HEAD\0symref=HEAD:refs/heads/main\n",
    );
  let started = false;
  await assert.rejects(
    lsRemote(
      undefined,
      "https://example.org/repo",
      false,
      false,
      true,
      async () =>
        new Response(
          new ReadableStream<Uint8Array>({
            pull(controller) {
              if (!started) {
                started = true;
                controller.enqueue(Buffer.from(prefix));
              } else
                controller.error(new Error("transport fault sensitive-token"));
            },
          }),
          {
            headers: {
              "content-type": "application/x-git-upload-pack-advertisement",
            },
          },
        ),
      { origin: "https://example.org", bearer: "sensitive-token" },
    ),
    (error: Error) =>
      error.message.includes("transport fault") &&
      !error.message.includes("sensitive-token"),
  );
});

test("discovery disposes an open body when the advertisement parser rejects", async () => {
  let disposed = 0;
  await assert.rejects(
    lsRemote(
      undefined,
      "https://example.org/repo",
      false,
      false,
      false,
      async () =>
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(Buffer.from("0011not a service"));
            },
            cancel() {
              disposed += 1;
            },
          }),
          {
            headers: {
              "content-type": "application/x-git-upload-pack-advertisement",
            },
          },
        ),
    ),
  );
  assert.equal(disposed, 1);
});

test("config actions preserve multi-valued refspecs and reject ambiguous writes", async (t) => {
  const repo = await fixture(t);
  const read = { add: false, getAll: false, unset: false, unsetAll: false };
  await configure(
    repo,
    "remote.origin.fetch",
    "+refs/heads/*:refs/remotes/origin/*",
    read,
  );
  await configure(repo, "remote.origin.fetch", "^refs/heads/private/*", {
    ...read,
    add: true,
  });
  await assert.rejects(
    configure(repo, "remote.origin.fetch", "replacement", read),
    /multiple values/,
  );
  assert.equal(
    (await configure(repo, "remote.origin.fetch", undefined, read)).value,
    "^refs/heads/private/*",
  );
  assert.deepEqual(
    (
      await configure(repo, "remote.origin.fetch", undefined, {
        ...read,
        getAll: true,
      })
    ).values,
    ["+refs/heads/*:refs/remotes/origin/*", "^refs/heads/private/*"],
  );
  await configure(repo, "remote.origin.fetch", undefined, {
    ...read,
    unsetAll: true,
  });
  assert.deepEqual(
    await git.getConfigAll({ fs, ...repo, path: "remote.origin.fetch" }),
    [],
  );
});

test("tracking uses configured mappings, reports asymmetric divergence, and respects exclusions", async (t) => {
  const repo = await fixture(t);
  const identity = {
    name: "Fixture",
    email: "fixture@example.org",
    timestamp: 1700000000,
    timezoneOffset: 0,
  };
  const tree = await git.writeTree({ fs, ...repo, tree: [] });
  const commit = (message: string, parent: string[]) =>
    git.writeCommit({
      fs,
      ...repo,
      commit: { tree, parent, author: identity, committer: identity, message },
    });
  const base = await commit("base", []);
  const local1 = await commit("local1", [base]);
  const local2 = await commit("local2", [local1]);
  const other = await commit("other", [base]);
  await git.writeRef({ fs, ...repo, ref: "refs/heads/main", value: local2 });
  await remoteAdd(repo, "origin", "https://example.org/repo");
  await localConfig(
    repo,
    "remote.origin.fetch",
    "+refs/heads/*:refs/remotes/origin/nested/*",
  );
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/remotes/origin/nested/main",
    value: other,
  });
  await setUpstream(repo, "main", "origin/nested/main");
  await git.setConfig({
    fs,
    ...repo,
    path: "branch.main.merge",
    value: "refs/heads/topic",
    append: true,
  });
  assert.deepEqual(await upstream(repo, "main"), {
    remote: "origin",
    merge: "refs/heads/main",
    ref: "refs/remotes/origin/nested/main",
  });
  await setUpstream(repo, "main", "origin/nested/main");
  assert.deepEqual(
    await git.getConfigAll({ fs, ...repo, path: "branch.main.merge" }),
    ["refs/heads/main"],
  );
  const branches = await trackedBranches(repo, false, true, 2);
  const main = branches.find(({ name }) => name === "main")!;
  assert.equal(main.ahead, 2);
  assert.equal(main.behind, 1);
  assert.equal(
    branches.find(({ name }) => name === "origin/nested/main")?.remote,
    true,
  );
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/remotes/origin/nested/topic",
    value: other,
  });
  assert.equal(
    (await trackingCheckoutTarget(repo, "topic"))?.ref,
    "refs/remotes/origin/nested/topic",
  );
  await git.writeRef({ fs, ...repo, ref: "refs/tags/topic", value: other });
  assert.equal(await trackingCheckoutTarget(repo, "topic"), undefined);
  await git.deleteRef({ fs, ...repo, ref: "refs/tags/topic" });
  await remoteAdd(repo, "shared", "https://other.org/repo");
  await localConfig(
    repo,
    "remote.shared.fetch",
    "+refs/heads/*:refs/remotes/origin/nested/*",
  );
  await assert.rejects(
    trackingCheckoutTarget(repo, "topic"),
    /multiple remote-tracking/,
  );
  await remoteRenameOrRemove(repo, "shared");
  await git.setConfig({
    fs,
    ...repo,
    path: "remote.origin.fetch",
    value: "^refs/heads/topic",
    append: true,
  });
  assert.equal(await trackingCheckoutTarget(repo, "topic"), undefined);
  await git.deleteRef({ fs, ...repo, ref: "refs/remotes/origin/nested/main" });
  assert.equal((await trackedBranches(repo, false, false, 2))[0]?.gone, true);
  await setUpstream(repo, "main", undefined);
  assert.equal(await upstream(repo, "main"), undefined);
});
