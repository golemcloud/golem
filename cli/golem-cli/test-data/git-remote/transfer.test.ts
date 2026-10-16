// Native subprocesses are deliberately confined to the CLI integration suite.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import {
  mkdtemp,
  readFile,
  rm,
  writeFile,
  mkdir,
  symlink,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  cloneCommand,
  fetchCommand,
} from "../../../../builtin-tools/git/src/transfer.ts";
import {
  repository,
  stage,
  createCommit,
  checkoutCommand,
  statusEntries,
} from "../../../../builtin-tools/git/src/git.ts";
import { pushCommand } from "../../../../builtin-tools/git/src/push.ts";
import {
  abortRebase,
  continueRebase,
  rebaseCommand,
  skipRebase,
} from "../../../../builtin-tools/git/src/rebase.ts";
import {
  abortMerge,
  mergeCommand,
  pullCommand,
} from "../../../../builtin-tools/git/src/integration.ts";

const git = process.env.GOLEM_TEST_GIT ?? "git";
async function run(
  cwd: string,
  args: string[],
  env: NodeJS.ProcessEnv = {},
  input?: Buffer,
) {
  return new Promise<Buffer>((resolve, reject) => {
    const child = spawn(git, args, {
      cwd,
      env: {
        ...process.env,
        GIT_CONFIG_NOSYSTEM: "1",
        GIT_CONFIG_GLOBAL: "/dev/null",
        GIT_TERMINAL_PROMPT: "0",
        ...env,
      },
      stdio: "pipe",
    });
    const out: Buffer[] = [],
      errors: Buffer[] = [];
    child.stdout.on("data", (bytes) => out.push(bytes));
    child.stderr.on("data", (bytes) => errors.push(bytes));
    child.on("error", reject);
    child.stdin.on("error", (error: NodeJS.ErrnoException) => {
      if (error.code !== "EPIPE") reject(error);
    });
    child.on("close", (code) =>
      code === 0
        ? resolve(Buffer.concat(out))
        : reject(new Error(Buffer.concat(errors).toString())),
    );
    child.stdin.end(input);
  });
}

async function fixture(t: test.TestContext) {
  assert.equal(
    (await run(process.cwd(), ["--version"])).toString().trim(),
    "git version 2.55.0",
    "differential tests require the pinned Git 2.55.0 reference (GOLEM_TEST_GIT)",
  );
  const root = await mkdtemp(path.join(os.tmpdir(), "golem-native-git-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = path.join(root, "source"),
    bare = path.join(root, "repo.git");
  await run(root, ["init", "--initial-branch=main", source]);
  await run(source, ["config", "user.name", "Author"]);
  await run(source, ["config", "user.email", "author@example.org"]);
  await writeFile(path.join(source, "hello.txt"), "first\n");
  await run(source, ["add", "."]);
  await run(source, ["commit", "-m", "first"]);
  await run(source, ["branch", "topic"]);
  await run(source, ["tag", "-a", "v1", "-m", "first release"]);
  await writeFile(path.join(source, "hello.txt"), "second\n");
  await run(source, ["commit", "-am", "second"]);
  await writeFile(
    path.join(source, "binary.dat"),
    Buffer.from([0, 255, 3, 128, 42]),
  );
  await run(source, ["add", "."]);
  await run(source, ["commit", "-m", "binary"]);
  await run(root, ["clone", "--bare", source, bare]);
  await run(bare, ["config", "http.receivepack", "true"]);
  const server = createServer(async (request, response) => {
    try {
      const chunks: Buffer[] = [];
      for await (const bytes of request) chunks.push(Buffer.from(bytes));
      const url = new URL(request.url!, "http://localhost");
      const bytes = await run(
        root,
        ["http-backend"],
        {
          GIT_PROJECT_ROOT: root,
          GIT_HTTP_EXPORT_ALL: "1",
          PATH_INFO: url.pathname,
          QUERY_STRING: url.search.slice(1),
          REQUEST_METHOD: request.method,
          GIT_PROTOCOL: request.headers["git-protocol"] as string | undefined,
          CONTENT_TYPE: request.headers["content-type"],
          CONTENT_LENGTH: String(Buffer.concat(chunks).length),
          REMOTE_USER: "fixture",
        },
        Buffer.concat(chunks),
      );
      const separator = bytes.indexOf("\r\n\r\n");
      assert.ok(separator >= 0, "CGI response must contain a header separator");
      for (const line of bytes
        .subarray(0, separator)
        .toString()
        .split("\r\n")) {
        const colon = line.indexOf(":");
        const key = line.slice(0, colon),
          value = line.slice(colon + 1).trim();
        if (key.toLowerCase() === "status")
          response.statusCode = Number(value.split(" ")[0]);
        else response.setHeader(key, value);
      }
      response.end(bytes.subarray(separator + 4));
    } catch (error) {
      response.statusCode = 500;
      response.end(error instanceof Error ? error.message : String(error));
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(
    () =>
      new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  );
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  const url = `http://127.0.0.1:${address.port}/repo.git`;
  return {
    root,
    source,
    bare,
    url,
    native: path.join(root, "native"),
    tool: path.join(root, "tool"),
  };
}

async function refs(dir: string) {
  return (await run(dir, ["for-each-ref", "--format=%(refname) %(objectname)"]))
    .toString()
    .trim()
    .split("\n")
    .filter(Boolean)
    .sort();
}
async function fetchHead(dir: string) {
  return (await readFile(path.join(dir, ".git/FETCH_HEAD"), "utf8"))
    .trim()
    .split("\n")
    .map((line) => line.split("\t").slice(0, 2).join("\t"))
    .sort();
}

test("native Git differential: clone default/upstream, tags, binary checkout and later fetch/prune", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  for (const key of [
    "branch.main.remote",
    "branch.main.merge",
    "remote.origin.fetch",
  ]) {
    assert.equal(
      (await run(f.tool, ["config", "--get", key])).toString(),
      (await run(f.native, ["config", "--get", key])).toString(),
    );
  }
  for (const [key, fallback] of [
    ["core.symlinks", "true"],
    ["core.ignorecase", "false"],
  ])
    assert.equal(
      (
        await run(f.tool, [
          "config",
          "--bool",
          "--default",
          fallback!,
          "--get",
          key!,
        ])
      ).toString(),
      (
        await run(f.native, [
          "config",
          "--bool",
          "--default",
          fallback!,
          "--get",
          key!,
        ])
      ).toString(),
    );
  for (const filename of ["hello.txt", "binary.dat"])
    assert.deepEqual(
      await readFile(path.join(f.tool, filename)),
      await readFile(path.join(f.native, filename)),
    );
  await run(f.bare, ["update-ref", "-d", "refs/heads/topic"]);
  await run(f.native, ["fetch", "--prune"]);
  await fetchCommand(
    await repository([f.tool]),
    undefined,
    [],
    { prune: true },
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  await run(f.tool, ["fsck", "--full"]);
});

test("native Git differential: shallow single branch, shared-history multi-ref deepen and unshallow", async (t) => {
  const f = await fixture(t);
  for (let i = 0; i < 3; i++) {
    await writeFile(path.join(f.source, "history.txt"), `${i}\n`);
    await run(f.source, ["add", "history.txt"]);
    await run(f.source, ["commit", "-m", `history ${i}`]);
  }
  await run(f.source, ["branch", "-f", "topic", "main"]);
  await run(f.source, ["push", f.bare, "main", "topic"]);
  await run(f.root, ["clone", "--depth", "1", "--no-tags", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", { depth: 1, tags: false }, fetch);
  const repo = await repository([f.tool]);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.equal(
    await readFile(path.join(repo.gitdir, "shallow"), "utf8"),
    await readFile(path.join(f.native, ".git/shallow"), "utf8"),
  );
  const refspec = "+refs/heads/*:refs/remotes/origin/*";
  await run(f.native, ["fetch", "--deepen", "1", "origin", refspec]);
  await fetchCommand(repo, undefined, [refspec], { deepen: 1 }, fetch);
  assert.equal(
    (await run(f.native, ["rev-list", "--count", "main"])).toString(),
    "2\n",
  );
  assert.equal(
    (await run(f.tool, ["rev-list", "--count", "main"])).toString(),
    "2\n",
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "shallow"), "utf8"),
    await readFile(path.join(f.native, ".git/shallow"), "utf8"),
  );
  await run(f.native, ["fetch", "--unshallow"]);
  await fetchCommand(repo, undefined, [], { unshallow: true }, fetch);
  assert.equal(
    (await run(f.tool, ["rev-parse", "--is-shallow-repository"])).toString(),
    "false\n",
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  await run(f.tool, ["fsck", "--full"]);
});

test("native Git differential: selecting an annotated tag yields a detached commit", async (t) => {
  const f = await fixture(t);
  await run(f.root, [
    "clone",
    "--branch",
    "v1",
    "--single-branch",
    "--no-tags",
    f.url,
    f.native,
  ]);
  await cloneCommand(
    f.root,
    f.url,
    "tool",
    { branch: "v1", singleBranch: true, tags: false },
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.equal(
    (await run(f.tool, ["rev-parse", "HEAD"])).toString(),
    (await run(f.native, ["rev-parse", "HEAD"])).toString(),
  );
  assert.equal(
    await readFile(path.join(f.tool, ".git/HEAD"), "utf8"),
    await readFile(path.join(f.native, ".git/HEAD"), "utf8"),
  );
  assert.equal(
    await readFile(path.join(f.tool, "hello.txt"), "utf8"),
    "first\n",
  );
  assert.equal(
    (await run(f.tool, ["config", "remote.origin.fetch"])).toString(),
    (await run(f.native, ["config", "remote.origin.fetch"])).toString(),
  );
});

test("released-library difference: empty clone cannot discover the remote unborn branch", async (t) => {
  const f = await fixture(t);
  for (const ref of await refs(f.bare))
    await run(f.bare, ["update-ref", "-d", ref.split(" ")[0]!]);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.equal(
    await readFile(path.join(f.tool, ".git/HEAD"), "utf8"),
    "ref: refs/heads/master\n",
  );
  assert.equal(
    await readFile(path.join(f.native, ".git/HEAD"), "utf8"),
    "ref: refs/heads/main\n",
  );
  await assert.rejects(run(f.tool, ["config", "branch.main.remote"]));
  assert.equal(
    (await run(f.tool, ["config", "remote.origin.fetch"])).toString(),
    "+refs/heads/*:refs/remotes/origin/*\n",
  );
});

test("native Git differential: initial fetch learns symbolic remote HEAD and explicit refspecs update tracking", async (t) => {
  const f = await fixture(t);
  for (const dir of [f.native, f.tool]) {
    await run(f.root, ["init", "--initial-branch=main", dir]);
    await run(dir, ["remote", "add", "origin", f.url]);
  }
  await run(f.native, ["fetch", "origin"]);
  await fetchCommand(await repository([f.tool]), "origin", [], {}, fetch);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  await run(f.native, ["fetch", "origin", "topic:refs/heads/local-topic"]);
  await fetchCommand(
    await repository([f.tool]),
    "origin",
    ["topic:refs/heads/local-topic"],
    {},
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
});

test("native Git differential: pruning packed refs supports both namespace transitions", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  for (const dir of [f.native, f.tool])
    await run(dir, ["pack-refs", "--all", "--prune"]);
  const topic = (await run(f.bare, ["rev-parse", "topic"])).toString().trim();
  await run(f.bare, ["update-ref", "-d", "refs/heads/topic"]);
  await run(f.bare, ["update-ref", "refs/heads/topic/subtopic", topic]);
  await run(f.native, ["fetch", "--prune"]);
  await fetchCommand(
    await repository([f.tool]),
    undefined,
    [],
    { prune: true },
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  await run(f.bare, ["update-ref", "-d", "refs/heads/topic/subtopic"]);
  await run(f.bare, ["update-ref", "refs/heads/topic", topic]);
  await run(f.native, ["fetch", "--prune"]);
  await fetchCommand(
    await repository([f.tool]),
    undefined,
    [],
    { prune: true },
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  await run(f.tool, ["fsck", "--full"]);
});

test("native Git differential: upstream outside tracking mapping, URL merge candidate and negative tags", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  for (const dir of [f.native, f.tool]) {
    await run(dir, [
      "config",
      "remote.origin.fetch",
      "+refs/heads/topic:refs/remotes/origin/topic",
    ]);
    await run(dir, ["update-ref", "-d", "refs/tags/v1"]);
  }
  await run(f.native, ["fetch"]);
  await fetchCommand(await repository([f.tool]), undefined, [], {}, fetch);
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  for (const dir of [f.native, f.tool])
    await run(dir, ["update-ref", "-d", "refs/tags/v1"]);
  await run(f.native, ["fetch", "--tags", "origin", "topic", "^refs/tags/v1"]);
  await fetchCommand(
    await repository([f.tool]),
    "origin",
    ["topic", "^refs/tags/v1"],
    { tags: true },
    fetch,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  await run(f.native, ["fetch", "--no-tags", f.url]);
  await fetchCommand(
    await repository([f.tool]),
    f.url,
    [],
    { tags: false },
    fetch,
  );
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
});

test("native Git differential: force cannot install an annotated tag as a local branch", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await assert.rejects(
    run(f.native, [
      "fetch",
      "--force",
      "origin",
      "+refs/tags/v1:refs/heads/new",
    ]),
  );
  await assert.rejects(
    fetchCommand(
      await repository([f.tool]),
      "origin",
      ["+refs/tags/v1:refs/heads/new"],
      { force: true },
      fetch,
    ),
    /non-commit branch/,
  );
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

test("native Git differential: push defaults, configured refspec precedence and triangular workflows", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  const before = await refs(f.bare);
  await run(f.native, ["push"]);
  const expected = await refs(f.bare);
  await restoreRefs(f.bare, before);
  await pushCommand(await repository([f.tool]), undefined, [], {}, fetch);
  assert.deepEqual(await refs(f.bare), expected);
  for (const dir of [f.native, f.tool])
    await run(dir, ["config", "branch.main.merge", "refs/heads/topic"]);
  await assert.rejects(run(f.native, ["push"]));
  await assert.rejects(
    pushCommand(await repository([f.tool]), undefined, [], {}, fetch),
    /same name/,
  );
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["config", "push.default", "nothing"]);
    await run(dir, ["config", "remote.origin.push", "main:configured"]);
  }
  const configuredBefore = await refs(f.bare);
  await run(f.native, ["push"]);
  const configuredAfter = await refs(f.bare);
  await restoreRefs(f.bare, configuredBefore);
  await pushCommand(await repository([f.tool]), undefined, [], {}, fetch);
  assert.deepEqual(await refs(f.bare), configuredAfter);
  await run(f.root, [
    "clone",
    "--bare",
    f.source,
    path.join(f.root, "mirror.git"),
  ]);
  const mirrorUrl = f.url.replace(/repo\.git$/, "mirror.git");
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["remote", "add", "mirror", mirrorUrl]);
    await run(dir, ["config", "push.default", "simple"]);
    await run(dir, ["config", "remote.pushDefault", "mirror"]);
  }
  const mirrorBefore = await refs(path.join(f.root, "mirror.git"));
  await run(f.native, ["push"]);
  const mirrorAfter = await refs(path.join(f.root, "mirror.git"));
  await restoreRefs(path.join(f.root, "mirror.git"), mirrorBefore);
  await pushCommand(await repository([f.tool]), undefined, [], {}, fetch);
  assert.deepEqual(await refs(path.join(f.root, "mirror.git")), mirrorAfter);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

test("native Git differential: an already up-to-date ref succeeds despite a stale tracking lease", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  const current = (await run(f.tool, ["rev-parse", "HEAD"])).toString().trim();
  await run(f.bare, ["fetch", f.native, "main"]);
  await run(f.bare, ["update-ref", "refs/heads/main", current]);
  await run(f.native, ["push", "--force-with-lease", "origin", "main"]);
  const outcome = await pushCommand(
    await repository([f.tool]),
    "origin",
    ["main"],
    { forceWithLease: true },
    fetch,
  );
  assert.ok(outcome.refs[0]!.upToDate);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

test("receive-pack old-OID CAS rejects a race after lease validation without overwriting the remote", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  const tracking = (await run(f.tool, ["rev-parse", "origin/main"]))
    .toString()
    .trim();
  const racing = (await run(f.bare, ["rev-parse", "topic"])).toString().trim();
  const race: typeof fetch = async (input, options) => {
    if (options?.method === "POST")
      await run(f.bare, ["update-ref", "refs/heads/main", racing]);
    return fetch(input, options);
  };
  await assert.rejects(
    pushCommand(
      await repository([f.tool]),
      "origin",
      ["main"],
      { forceWithLease: true },
      race,
    ),
    /incorrect old value/,
  );
  assert.equal(
    (await run(f.bare, ["rev-parse", "main"])).toString().trim(),
    racing,
  );
  assert.equal(
    (await run(f.tool, ["rev-parse", "origin/main"])).toString().trim(),
    tracking,
  );
});

test("released-library difference: multi-ref push fails before sending, instead of partial server acceptance", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  await run(f.bare, ["config", "receive.denyDeletes", "true"]);
  const before = await refs(f.bare);
  const refspecs = ["main:accepted", ":refs/heads/topic"];
  await assert.rejects(
    run(f.native, ["push", "--atomic", "origin", ...refspecs]),
  );
  await assert.rejects(
    pushCommand(
      await repository([f.tool]),
      "origin",
      refspecs,
      { atomic: true },
      fetch,
    ),
  );
  assert.deepEqual(await refs(f.bare), before);
  await assert.rejects(run(f.native, ["push", "origin", ...refspecs]));
  const after = await refs(f.bare);
  await restoreRefs(f.bare, before);
  await assert.rejects(
    pushCommand(await repository([f.tool]), "origin", refspecs, {}, fetch),
    /multi-ref push is unsupported/,
  );
  assert.notDeepEqual(after, before);
  assert.deepEqual(await refs(f.bare), before);
});

async function restoreRefs(dir: string, original: string[]) {
  for (const line of await refs(dir))
    await run(dir, ["update-ref", "-d", line.split(" ")[0]!]);
  for (const line of original) {
    const [ref, oid] = line.split(" ");
    await run(dir, ["update-ref", ref!, oid!]);
  }
}

async function commitBoth(f: Awaited<ReturnType<typeof fixture>>) {
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["config", "user.name", "Push Author"]);
    await run(dir, ["config", "user.email", "push@example.org"]);
    await writeFile(path.join(dir, "hello.txt"), "pushed contents\n");
    await run(dir, ["commit", "-am", "push transfer"], {
      GIT_AUTHOR_DATE: "2024-01-02T03:04:05Z",
      GIT_COMMITTER_DATE: "2024-01-02T03:04:05Z",
    });
  }
  assert.equal(
    (await run(f.tool, ["rev-parse", "HEAD"])).toString(),
    (await run(f.native, ["rev-parse", "HEAD"])).toString(),
  );
}

test("released-library difference: atomic push fails without writes; separate single-ref pushes and deletes work", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["branch", "topic", "origin/topic"]);
    await run(dir, ["tag", "v2", "HEAD"]);
  }
  const before = await refs(f.bare);
  const refspecs = ["main:renamed", "topic:new-topic", "refs/tags/v2"];
  await run(f.native, ["push", "--atomic", "-u", "origin", ...refspecs]);
  const expected = await refs(f.bare);
  await restoreRefs(f.bare, before);
  await assert.rejects(
    pushCommand(
      await repository([f.tool]),
      "origin",
      refspecs,
      { atomic: true, setUpstream: true },
      fetch,
    ),
    /atomic push is unsupported/,
  );
  assert.deepEqual(await refs(f.bare), before);
  for (const spec of refspecs) {
    const result = await pushCommand(
      await repository([f.tool]),
      "origin",
      [spec],
      { setUpstream: true },
      fetch,
    );
    assert.ok(result.refs[0]!.ok && !result.refs[0]!.upToDate);
  }
  assert.deepEqual(await refs(f.bare), expected);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  for (const key of [
    "branch.main.remote",
    "branch.main.merge",
    "branch.topic.remote",
    "branch.topic.merge",
  ]) {
    assert.equal(
      (await run(f.tool, ["config", key])).toString(),
      (await run(f.native, ["config", key])).toString(),
    );
  }
  await run(f.bare, ["fsck", "--full"]);
  await run(f.native, [
    "push",
    "--delete",
    "origin",
    "renamed",
    "new-topic",
    "v2",
  ]);
  const deleted = await refs(f.bare);
  await restoreRefs(f.bare, expected);
  for (const ref of [
    "refs/heads/renamed",
    "refs/heads/new-topic",
    "refs/tags/v2",
  ])
    await pushCommand(
      await repository([f.tool]),
      "origin",
      [ref],
      { delete: true },
      fetch,
    );
  assert.deepEqual(await refs(f.bare), deleted);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

test("native Git differential: stale leases never refresh themselves and explicit leases allow a matching force", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  const before = await refs(f.bare);
  const old = (await run(f.bare, ["rev-parse", "topic"])).toString().trim();
  await run(f.bare, ["update-ref", "refs/heads/main", old]);
  await assert.rejects(
    run(f.native, ["push", "--force-with-lease", "origin", "main"]),
  );
  await assert.rejects(
    pushCommand(
      await repository([f.tool]),
      "origin",
      ["main"],
      { forceWithLease: true },
      fetch,
    ),
    /stale force-with-lease/,
  );
  assert.equal(
    (await run(f.bare, ["rev-parse", "main"])).toString().trim(),
    old,
  );
  await run(f.native, [
    "push",
    `--force-with-lease=refs/heads/main:${old}`,
    "origin",
    "main",
  ]);
  const expected = await refs(f.bare);
  await restoreRefs(f.bare, before);
  await run(f.bare, ["update-ref", "refs/heads/main", old]);
  await pushCommand(
    await repository([f.tool]),
    "origin",
    ["main"],
    { forceWithLease: true, lease: `refs/heads/main:${old}` },
    fetch,
  );
  assert.deepEqual(await refs(f.bare), expected);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

test("native Git differential: explicit force overrides a stale lease only for forced refs", async (t) => {
  for (const flag of [false, true]) {
    await t.test(flag ? "force flag" : "forced refspec", async (t) => {
      const f = await fixture(t);
      await run(f.root, ["clone", f.url, f.native]);
      await cloneCommand(f.root, f.url, "tool", {}, fetch);
      await commitBoth(f);
      for (const dir of [f.native, f.tool])
        await run(dir, ["branch", "topic", "origin/topic"]);
      const previousMain = (await run(f.bare, ["rev-parse", "main"]))
        .toString()
        .trim();
      const old = (await run(f.bare, ["rev-parse", "topic"])).toString().trim();
      await run(f.bare, ["update-ref", "refs/heads/main", old]);
      await run(f.bare, ["update-ref", "refs/heads/topic", previousMain]);
      const before = await refs(f.bare);
      const spec = flag ? "main" : "+main";
      const native = run(f.native, [
        "push",
        "--force-with-lease",
        ...(flag ? ["--force"] : []),
        "origin",
        spec,
        "topic",
      ]);
      if (flag) await native;
      else await assert.rejects(native, /stale info/);
      const expected = await refs(f.bare);
      await restoreRefs(f.bare, before);
      const tool = pushCommand(
        await repository([f.tool]),
        "origin",
        [spec],
        { forceWithLease: true, force: flag },
        fetch,
      );
      await tool;
      if (flag)
        await pushCommand(
          await repository([f.tool]),
          "origin",
          ["topic"],
          { forceWithLease: true, force: true },
          fetch,
        );
      else
        await assert.rejects(
          pushCommand(
            await repository([f.tool]),
            "origin",
            ["topic"],
            { forceWithLease: true },
            fetch,
          ),
          /stale force-with-lease/,
        );
      assert.deepEqual(await refs(f.bare), expected);
      assert.deepEqual(await refs(f.tool), await refs(f.native));
    });
  }
});

test("native Git differential: tags-only push ignores configured forced branch refspecs", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["tag", "v2"]);
    await run(dir, ["config", "remote.origin.push", "+main:refs/heads/topic"]);
  }
  const before = await refs(f.bare);
  await run(f.native, ["push", "--tags", "origin"]);
  const expected = await refs(f.bare);
  assert.equal(
    expected.find((ref) => ref.startsWith("refs/heads/topic ")),
    before.find((ref) => ref.startsWith("refs/heads/topic ")),
  );
  await restoreRefs(f.bare, before);
  // Only one local tag remains: --tags must not pick the configured branch refspec.
  await run(f.tool, ["tag", "-d", "v1"]);
  await pushCommand(
    await repository([f.tool]),
    "origin",
    [],
    { tags: true },
    fetch,
  );
  assert.deepEqual(await refs(f.bare), expected);
  assert.deepEqual(
    (await refs(f.tool)).filter((ref) => !ref.startsWith("refs/tags/v1 ")),
    (await refs(f.native)).filter((ref) => !ref.startsWith("refs/tags/v1 ")),
  );
});

test("native Git differential: source ambiguity rejects loose and packed refs; qualification and strong refs disambiguate", async (t) => {
  for (const packed of [false, true]) {
    await t.test(packed ? "packed" : "loose", async (t) => {
      const f = await fixture(t);
      await run(f.root, ["clone", f.url, f.native]);
      await cloneCommand(f.root, f.url, "tool", {}, fetch);
      await commitBoth(f);
      for (const dir of [f.native, f.tool]) {
        await run(dir, ["branch", "release"]);
        await run(dir, ["tag", "release", "origin/topic"]);
        await run(dir, ["branch", "origin/main"]);
        if (packed) await run(dir, ["pack-refs", "--all", "--prune"]);
      }
      const before = await refs(f.bare);
      await assert.rejects(
        run(f.native, ["push", "origin", "release:refs/heads/deploy"]),
        /matches more than one/,
      );
      await assert.rejects(
        pushCommand(
          await repository([f.tool]),
          "origin",
          ["release:refs/heads/deploy"],
          {},
          fetch,
        ),
        /matches more than one/,
      );
      assert.deepEqual(await refs(f.bare), before);
      const specs = [
        "refs/heads/release:refs/heads/deploy",
        "origin/main:refs/heads/strong",
      ];
      await run(f.native, ["push", "origin", ...specs]);
      const expected = await refs(f.bare);
      await restoreRefs(f.bare, before);
      for (const spec of specs)
        await pushCommand(
          await repository([f.tool]),
          "origin",
          [spec],
          {},
          fetch,
        );
      assert.deepEqual(await refs(f.bare), expected);
      assert.deepEqual(await refs(f.tool), await refs(f.native));
    });
  }
});

test("native Git differential: invalid refspecs, absent deletion and missing upstream fail without changing refs", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  const before = await refs(f.bare);
  for (const spec of [
    "main:",
    ":missing",
    "refs/heads/absent**:refs/heads/new**",
  ]) {
    await assert.rejects(run(f.native, ["push", "origin", spec]));
    await assert.rejects(
      pushCommand(await repository([f.tool]), "origin", [spec], {}, fetch),
    );
    assert.deepEqual(await refs(f.bare), before);
  }
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["remote", "rename", "origin", "only"]);
    await run(dir, ["branch", "--unset-upstream"]);
  }
  await assert.rejects(run(f.native, ["push"]), /no upstream branch/);
  await assert.rejects(
    pushCommand(await repository([f.tool]), undefined, [], {}, fetch),
    /no single upstream/,
  );
  assert.deepEqual(await refs(f.bare), before);
});

test("released-library difference: multi-ref all/tags and matching push reject without remote writes", async (t) => {
  for (const mode of ["all", "tags", "matching"] as const) {
    await t.test(mode, async (t) => {
      const f = await fixture(t);
      await run(f.root, ["clone", f.url, f.native]);
      await cloneCommand(f.root, f.url, "tool", {}, fetch);
      await commitBoth(f);
      for (const dir of [f.native, f.tool]) {
        await run(dir, ["branch", "fresh"]);
        await run(dir, ["tag", "v2"]);
        if (mode === "matching")
          await run(dir, ["config", "push.default", "matching"]);
      }
      const before = await refs(f.bare);
      await run(f.native, [
        "push",
        ...(mode === "matching" ? [] : [`--${mode}`]),
        "origin",
      ]);
      const expected = await refs(f.bare);
      await restoreRefs(f.bare, before);
      await assert.rejects(
        pushCommand(
          await repository([f.tool]),
          "origin",
          [],
          mode === "matching" ? {} : { [mode]: true },
          fetch,
        ),
        /(?:multi-ref|matching) push is unsupported/,
      );
      assert.notDeepEqual(expected, before);
      assert.deepEqual(await refs(f.bare), before);
      await run(f.bare, ["fsck", "--full"]);
    });
  }
});

test("native Git differential: an explicit lease does not force unrelated refs", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  await commitBoth(f);
  for (const dir of [f.native, f.tool])
    await run(dir, ["branch", "topic", "origin/topic"]);
  const old = (await run(f.bare, ["rev-parse", "main"])).toString().trim();
  await run(f.bare, ["update-ref", "refs/heads/topic", old]);
  const before = await refs(f.bare);
  await assert.rejects(
    run(f.native, [
      "push",
      `--force-with-lease=refs/heads/main:${old}`,
      "origin",
      "main",
      "topic",
    ]),
  );
  const expected = await refs(f.bare);
  await restoreRefs(f.bare, before);
  await pushCommand(
    await repository([f.tool]),
    "origin",
    ["main"],
    { forceWithLease: true, lease: `refs/heads/main:${old}` },
    fetch,
  );
  await assert.rejects(
    pushCommand(
      await repository([f.tool]),
      "origin",
      ["topic"],
      { forceWithLease: true, lease: `refs/heads/main:${old}` },
      fetch,
    ),
    /not a simple fast-forward/,
  );
  assert.deepEqual(await refs(f.bare), expected);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

async function mergeFixture(
  t: test.TestContext,
  base: (dir: string) => Promise<void>,
  ours: (dir: string) => Promise<void>,
  theirs: (dir: string) => Promise<void>,
) {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  const identities = {
    GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z",
    GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z",
  };
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["config", "user.name", "Author"]);
    await run(dir, ["config", "user.email", "author@example.org"]);
    await base(dir);
    await run(dir, ["add", "."]);
    await run(dir, ["commit", "--allow-empty", "-m", "merge base"], identities);
    await run(dir, ["branch", "topic"]);
    await ours(dir);
    await run(dir, ["add", "."]);
    await run(dir, ["commit", "--allow-empty", "-m", "ours"], identities);
    await run(dir, ["checkout", "topic"]);
    await theirs(dir);
    await run(dir, ["add", "."]);
    await run(dir, ["commit", "--allow-empty", "-m", "theirs"], identities);
    await run(dir, ["checkout", "main"]);
  }
  assert.deepEqual(await refs(f.native), await refs(f.tool));
  return f;
}

async function compareMergeState(f: Awaited<ReturnType<typeof fixture>>) {
  assert.deepEqual(
    (await run(f.tool, ["ls-files", "--stage"])).toString(),
    (await run(f.native, ["ls-files", "--stage"])).toString(),
  );
  const expected = (
    await run(f.native, ["status", "--porcelain=v1", "--untracked-files=all"])
  )
    .toString()
    .trimEnd()
    .split("\n")
    .filter(Boolean)
    .map((line) => [line.slice(3), line.slice(0, 2)]);
  assert.deepEqual(
    (await statusEntries(await repository([f.tool]), [])).map((e) => [
      e.path,
      e.code,
    ]),
    expected,
  );
  assert.equal(
    (await run(f.tool, ["rev-parse", "ORIG_HEAD"])).toString(),
    (await run(f.native, ["rev-parse", "ORIG_HEAD"])).toString(),
  );
}

test("native Git differential: complete conflicting merge index, resolution commit parents and tree", async (t) => {
  const f = await mergeFixture(
    t,
    (dir) => writeFile(path.join(dir, "text"), "base\n"),
    (dir) => writeFile(path.join(dir, "text"), "ours\n"),
    async (dir) => {
      await writeFile(path.join(dir, "text"), "theirs\n");
      await writeFile(path.join(dir, "addition"), "incoming\n");
    },
  );
  await assert.rejects(run(f.native, ["merge", "topic"]));
  await assert.rejects(
    mergeCommand(await repository([f.tool]), "topic"),
    /merge conflicts/,
  );
  await compareMergeState(f);
  assert.equal(
    await readFile(path.join(f.tool, "text"), "utf8"),
    await readFile(path.join(f.native, "text"), "utf8"),
  );
  assert.equal(
    await readFile(path.join(f.tool, ".git/MERGE_HEAD"), "utf8"),
    await readFile(path.join(f.native, ".git/MERGE_HEAD"), "utf8"),
  );
  for (const dir of [f.native, f.tool])
    await writeFile(path.join(dir, "text"), "base\n");
  await run(f.native, ["add", "text"]);
  await run(f.native, ["commit", "-m", "resolution"]);
  const repo = await repository([f.tool]);
  await stage(repo, ["text"], false, false);
  await createCommit(repo, ["resolution"], undefined, false);
  assert.equal(
    (await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
    (await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
  );
});

for (const kind of [
  "binary",
  "symlink",
  "delete",
  "type",
  "reverse-type",
  "unterminated",
  "directory",
] as const) {
  test(`native Git differential: ${kind} conflict stages/worktree and abort`, async (t) => {
    const f = await mergeFixture(
      t,
      (dir) =>
        kind === "symlink" || kind === "reverse-type"
          ? symlink("base", path.join(dir, "conflict"))
          : writeFile(
              path.join(dir, "conflict"),
              kind === "binary" ? Buffer.from([0, 255, 1]) : "base\n",
            ),
      async (dir) => {
        if (kind === "symlink" || kind === "reverse-type") {
          await rm(path.join(dir, "conflict"));
          await symlink("ours", path.join(dir, "conflict"));
        } else
          await writeFile(
            path.join(dir, "conflict"),
            kind === "binary"
              ? Buffer.from([0, 255, 2])
              : kind === "unterminated"
                ? "ours"
                : "ours\n",
          );
      },
      async (dir) => {
        if (
          ["symlink", "delete", "type", "reverse-type", "directory"].includes(
            kind,
          )
        )
          await rm(path.join(dir, "conflict"));
        if (kind === "symlink" || kind === "type")
          await symlink("theirs", path.join(dir, "conflict"));
        else if (kind === "directory") {
          await mkdir(path.join(dir, "conflict"));
          await writeFile(path.join(dir, "conflict/nested"), "theirs\n");
        } else if (kind === "binary")
          await writeFile(path.join(dir, "conflict"), Buffer.from([0, 255, 3]));
        else if (kind === "reverse-type" || kind === "unterminated")
          await writeFile(path.join(dir, "conflict"), "theirs");
      },
    );
    await assert.rejects(run(f.native, ["merge", "topic"]));
    await assert.rejects(
      mergeCommand(await repository([f.tool]), "topic"),
      /merge conflicts/,
    );
    await compareMergeState(f);
    assert.equal(
      (await run(f.tool, ["diff", "--no-ext-diff"])).toString(),
      (await run(f.native, ["diff", "--no-ext-diff"])).toString(),
    );
    await run(f.native, ["merge", "--abort"]);
    await abortMerge(await repository([f.tool]));
    assert.deepEqual(await refs(f.tool), await refs(f.native));
    assert.equal(
      (await run(f.tool, ["status", "--porcelain"])).toString(),
      (await run(f.native, ["status", "--porcelain"])).toString(),
    );
  });
}

test("native Git differential: clean three-way merge includes both sides and preserves unrelated dirty worktree", async (t) => {
  const f = await mergeFixture(
    t,
    async (dir) => {
      await writeFile(path.join(dir, "text"), "one\ntwo\nthree\nfour\nfive\n");
    },
    (dir) =>
      writeFile(path.join(dir, "text"), "ours\ntwo\nthree\nfour\nfive\n"),
    (dir) =>
      writeFile(path.join(dir, "text"), "one\ntwo\nthree\nfour\ntheirs\n"),
  );
  for (const dir of [f.native, f.tool])
    await writeFile(path.join(dir, "hello.txt"), "dirty unrelated\n");
  await run(f.native, ["merge", "topic"]);
  await mergeCommand(await repository([f.tool]), "topic");
  assert.equal(
    (await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
    (await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
  );
  await compareMergeState(f);
  assert.equal(
    await readFile(path.join(f.tool, "text"), "utf8"),
    await readFile(path.join(f.native, "text"), "utf8"),
  );
});

test("native Git differential: pull fast-forward preserves unrelated staged changes without identity", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  for (const dir of [f.native, f.tool]) {
    await writeFile(path.join(dir, "staged"), "local staged\n");
    await run(dir, ["add", "staged"]);
    await writeFile(path.join(dir, "hello.txt"), "unrelated dirty\n");
  }
  await writeFile(path.join(f.source, "incoming"), "remote addition\n");
  await run(f.source, ["add", "."]);
  await run(f.source, ["commit", "-m", "remote addition"]);
  await run(f.bare, ["fetch", f.source, "+main:main"]);
  await run(f.native, ["pull"]);
  await pullCommand(await repository([f.tool]), undefined, [], {}, fetch);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  await compareMergeState(f);
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
});

for (const strategy of [
  "default",
  "no-rebase",
  "ff-only",
  "ff",
  "no-ff",
  "config",
  "rebase",
  "rebase-config",
] as const) {
  test(`native Git differential: divergent pull ${strategy}`, async (t) => {
    const f = await mergeFixture(
      t,
      async () => {},
      (dir) => writeFile(path.join(dir, "local"), "local change\n"),
      (dir) => writeFile(path.join(dir, "remote"), "remote change\n"),
    );
    const theirs = (await run(f.native, ["rev-parse", "topic"]))
      .toString()
      .trim();
    await run(f.bare, ["fetch", f.native, "+topic:main"]);
    if (strategy === "config") {
      for (const dir of [f.native, f.tool])
        await run(dir, ["config", "pull.rebase", "false"]);
    }
    if (strategy === "rebase-config") {
      for (const dir of [f.native, f.tool])
        await run(dir, ["config", "branch.main.rebase", "true"]);
    }
    const args =
      strategy === "default" ||
      strategy === "config" ||
      strategy === "rebase-config"
        ? []
        : [`--${strategy}`];
    const nativeResult = await run(f.native, ["pull", ...args]).then(
      () => "success",
      (error) => String(error),
    );
    const toolResult = await pullCommand(
      await repository([f.tool]),
      undefined,
      [],
      {
        rebase:
          strategy === "no-rebase"
            ? false
            : strategy === "rebase"
              ? true
              : undefined,
        ffOnly: strategy === "ff-only",
        ff: strategy === "ff",
        noFf: strategy === "no-ff",
      },
      fetch,
    ).then(
      () => "success",
      (error) => String(error),
    );
    assert.equal(
      toolResult === "success",
      nativeResult === "success",
      `native: ${nativeResult}; tool: ${toolResult}`,
    );
    if (
      toolResult === "success" &&
      (strategy === "rebase" || strategy === "rebase-config")
    ) {
      assert.equal(
        (
          await run(f.tool, [
            "show",
            "-s",
            "--format=%T %P %s %an <%ae> %at",
            "HEAD",
          ])
        ).toString(),
        (
          await run(f.native, [
            "show",
            "-s",
            "--format=%T %P %s %an <%ae> %at",
            "HEAD",
          ])
        ).toString(),
      );
      assert.equal(
        (await run(f.tool, ["rev-parse", "HEAD^"])).toString().trim(),
        theirs,
      );
    } else if (toolResult === "success") {
      assert.equal(
        (
          await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])
        ).toString(),
        (
          await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])
        ).toString(),
      );
      assert.equal(
        (await run(f.tool, ["rev-parse", "HEAD^2"])).toString().trim(),
        theirs,
      );
    } else assert.deepEqual(await refs(f.tool), await refs(f.native));
    assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  });
}

test("native Git differential: fast-forward untracked collision and explicit no-ff/config override", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  const original = (await run(f.native, ["rev-parse", "HEAD"]))
    .toString()
    .trim();
  await writeFile(path.join(f.source, "incoming"), "identical content\n");
  await run(f.source, ["add", "."]);
  await run(f.source, ["commit", "-m", "remote addition"]);
  await run(f.bare, ["fetch", f.source, "+main:main"]);
  await run(f.native, ["fetch"]);
  await fetchCommand(await repository([f.tool]), undefined, [], {}, fetch);
  for (const dir of [f.native, f.tool])
    await writeFile(path.join(dir, "incoming"), "identical content\n");
  const nativeSuccess = await run(f.native, ["merge", "origin/main"]).then(
    () => true,
    () => false,
  );
  const toolSuccess = await mergeCommand(
    await repository([f.tool]),
    "origin/main",
  ).then(
    () => true,
    () => false,
  );
  assert.equal(toolSuccess, nativeSuccess);
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["reset", "--hard", original]);
    await rm(path.join(dir, "incoming"), { force: true });
    await run(dir, ["config", "user.name", "Author"]);
    await run(dir, ["config", "user.email", "author@example.org"]);
    await run(dir, ["config", "merge.ff", "only"]);
  }
  await run(f.native, ["merge", "--no-ff", "origin/main"]);
  await mergeCommand(await repository([f.tool]), "origin/main", { noFf: true });
  assert.equal(
    (await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
    (await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
  );
});

test("native Git differential: abort preserves dirty resolution matching original index but protects dirty incoming paths", async (t) => {
  const f = await mergeFixture(
    t,
    (dir) => writeFile(path.join(dir, "text"), "base\n"),
    (dir) => writeFile(path.join(dir, "text"), "ours\n"),
    async (dir) => {
      await writeFile(path.join(dir, "text"), "theirs\n");
      await writeFile(path.join(dir, "incoming"), "incoming\n");
    },
  );
  await assert.rejects(run(f.native, ["merge", "topic"]));
  await assert.rejects(
    mergeCommand(await repository([f.tool]), "topic"),
    /merge conflicts/,
  );
  for (const dir of [f.native, f.tool]) {
    await writeFile(path.join(dir, "text"), "ours\n");
    await run(dir, ["add", "text"]);
    await writeFile(path.join(dir, "text"), "new dirty resolution\n");
    await writeFile(path.join(dir, "incoming"), "dirty incoming\n");
  }
  await assert.rejects(run(f.native, ["merge", "--abort"]));
  await assert.rejects(
    abortMerge(await repository([f.tool])),
    /prevent merge abort/,
  );
  for (const dir of [f.native, f.tool])
    await writeFile(path.join(dir, "incoming"), "incoming\n");
  // Native abort uses cached stat data; refresh the restored incoming file.
  await run(f.native, ["status", "--porcelain"]);
  await run(f.native, ["merge", "--abort"]);
  await abortMerge(await repository([f.tool]));
  assert.equal(
    await readFile(path.join(f.tool, "text"), "utf8"),
    "new dirty resolution\n",
  );
  assert.equal(
    (await run(f.tool, ["status", "--porcelain"])).toString(),
    (await run(f.native, ["status", "--porcelain"])).toString(),
  );
});

test("native Git differential: pull into an unborn branch", async (t) => {
  const f = await fixture(t);
  for (const dir of [f.native, f.tool]) {
    await run(f.root, ["init", "--initial-branch=main", dir]);
    await run(dir, ["remote", "add", "origin", f.url]);
  }
  await run(f.native, ["pull", "origin", "main"]);
  await pullCommand(await repository([f.tool]), "origin", ["main"], {}, fetch);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
  assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
  assert.equal(
    (await run(f.tool, ["ls-files", "--stage"])).toString(),
    (await run(f.native, ["ls-files", "--stage"])).toString(),
  );
});

test("native Git differential: fast-forward dirty tracked content matching incoming tree", async (t) => {
  const f = await fixture(t);
  await run(f.root, ["clone", f.url, f.native]);
  await cloneCommand(f.root, f.url, "tool", {}, fetch);
  for (const dir of [f.native, f.tool, f.source])
    await writeFile(path.join(dir, "hello.txt"), "identical incoming change\n");
  await run(f.source, ["commit", "-am", "incoming tracked"]);
  await run(f.bare, ["fetch", f.source, "+main:main"]);
  await run(f.native, ["fetch"]);
  await fetchCommand(await repository([f.tool]), undefined, [], {}, fetch);
  const nativeSuccess = await run(f.native, ["merge", "origin/main"]).then(
    () => true,
    () => false,
  );
  const toolSuccess = await mergeCommand(
    await repository([f.tool]),
    "origin/main",
  ).then(
    () => true,
    () => false,
  );
  assert.equal(toolSuccess, nativeSuccess);
  assert.deepEqual(await refs(f.tool), await refs(f.native));
});

for (const local of [false, true]) {
  test(`native Git differential: conflicted pull ${local ? "local upstream" : "HTTP upstream"} and resolved checkout lifecycle`, async (t) => {
    const f = await mergeFixture(
      t,
      (dir) => writeFile(path.join(dir, "text"), "base\n"),
      (dir) => writeFile(path.join(dir, "text"), "ours\n"),
      (dir) => writeFile(path.join(dir, "text"), "theirs\n"),
    );
    if (local) {
      for (const dir of [f.native, f.tool]) {
        await run(dir, ["config", "branch.main.remote", "."]);
        await run(dir, ["config", "branch.main.merge", "refs/heads/topic"]);
      }
    } else await run(f.bare, ["fetch", f.native, "+topic:main"]);
    await assert.rejects(run(f.native, ["pull", "--no-rebase"]));
    await assert.rejects(
      pullCommand(
        await repository([f.tool]),
        undefined,
        [],
        { rebase: false },
        fetch,
      ),
      /merge conflicts/,
    );
    await compareMergeState(f);
    assert.equal(
      await readFile(path.join(f.tool, "text"), "utf8"),
      await readFile(path.join(f.native, "text"), "utf8"),
    );
    for (const dir of [f.native, f.tool])
      await writeFile(path.join(dir, "text"), "resolved\n");
    await run(f.native, ["add", "text"]);
    await stage(await repository([f.tool]), ["text"], false, false);
    await run(f.native, ["checkout", "-b", "resolution"]);
    await checkoutCommand(
      await repository([f.tool]),
      "resolution",
      false,
      undefined,
      [],
    );
    for (const dir of [f.native, f.tool])
      await assert.rejects(readFile(path.join(dir, ".git/MERGE_HEAD")), {
        code: "ENOENT",
      });
    await run(f.native, ["commit", "-m", "resolution without merge state"]);
    await createCommit(
      await repository([f.tool]),
      ["resolution without merge state"],
      undefined,
      false,
    );
    assert.equal(
      (await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
      (
        await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])
      ).toString(),
    );
  });
}

for (const kind of ["delete", "binary", "symlink"] as const) {
  test(`native Git differential: ${kind} conflict preserves a dirty worktree result equal to ours`, async (t) => {
    const f = await mergeFixture(
      t,
      (dir) =>
        kind === "symlink"
          ? symlink("base", path.join(dir, "conflict"))
          : writeFile(
              path.join(dir, "conflict"),
              kind === "binary" ? Buffer.from([0, 255, 1]) : "base\n",
            ),
      async (dir) => {
        if (kind === "symlink") {
          await rm(path.join(dir, "conflict"));
          await symlink("ours", path.join(dir, "conflict"));
        } else
          await writeFile(
            path.join(dir, "conflict"),
            kind === "binary" ? Buffer.from([0, 255, 2]) : "ours\n",
          );
      },
      async (dir) => {
        if (kind === "delete" || kind === "symlink")
          await rm(path.join(dir, "conflict"));
        if (kind === "symlink")
          await symlink("theirs", path.join(dir, "conflict"));
        if (kind === "binary")
          await writeFile(path.join(dir, "conflict"), Buffer.from([0, 255, 3]));
      },
    );
    for (const dir of [f.native, f.tool]) {
      if (kind === "symlink") {
        await rm(path.join(dir, "conflict"));
        await symlink("dirty", path.join(dir, "conflict"));
      } else
        await writeFile(
          path.join(dir, "conflict"),
          kind === "binary" ? Buffer.from([0, 255, 9]) : "dirty\n",
        );
    }
    await assert.rejects(run(f.native, ["merge", "topic"]));
    await assert.rejects(
      mergeCommand(await repository([f.tool]), "topic"),
      /merge conflicts/,
    );
    await compareMergeState(f);
    assert.equal(
      (await run(f.tool, ["diff", "--no-ext-diff"])).toString(),
      (await run(f.native, ["diff", "--no-ext-diff"])).toString(),
    );
    await run(f.native, ["merge", "--abort"]);
    await abortMerge(await repository([f.tool]));
    assert.equal(
      (await run(f.tool, ["diff", "--no-ext-diff"])).toString(),
      (await run(f.native, ["diff", "--no-ext-diff"])).toString(),
    );
  });
}

for (const explicit of [false, true]) {
  test(`native Git differential: pull unmapped configured upstream ${explicit ? "explicit branch" : "default branch"}`, async (t) => {
    const f = await fixture(t);
    await run(f.root, ["clone", f.url, f.native]);
    await cloneCommand(f.root, f.url, "tool", {}, fetch);
    for (const dir of [f.native, f.tool])
      await run(dir, [
        "config",
        "remote.origin.fetch",
        "+refs/heads/topic:refs/remotes/origin/topic",
      ]);
    await writeFile(path.join(f.source, "incoming"), "unmapped upstream\n");
    await run(f.source, ["add", "."]);
    await run(f.source, ["commit", "-m", "unmapped upstream"]);
    await run(f.bare, ["fetch", f.source, "+main:main"]);
    await run(f.native, [
      "pull",
      "--no-rebase",
      ...(explicit ? ["origin", "main"] : []),
    ]);
    await pullCommand(
      await repository([f.tool]),
      explicit ? "origin" : undefined,
      explicit ? ["main"] : [],
      { rebase: false },
      fetch,
    );
    assert.deepEqual(await refs(f.tool), await refs(f.native));
    assert.deepEqual(await fetchHead(f.tool), await fetchHead(f.native));
    assert.equal(
      await readFile(path.join(f.tool, "incoming"), "utf8"),
      "unmapped upstream\n",
    );
  });
}

test("native Git differential: unchanged ours resolves an add/add binary conflict", async (t) => {
  const f = await mergeFixture(
    t,
    async () => {},
    (dir) => writeFile(path.join(dir, "conflict"), Buffer.from([0, 255, 2])),
    (dir) => writeFile(path.join(dir, "conflict"), Buffer.from([0, 255, 3])),
  );
  await assert.rejects(run(f.native, ["merge", "topic"]));
  await assert.rejects(
    mergeCommand(await repository([f.tool]), "topic"),
    /merge conflicts/,
  );
  await run(f.native, ["add", "conflict"]);
  await stage(await repository([f.tool]), ["conflict"], false, false);
  await run(f.native, ["commit", "-m", "accept ours"]);
  await createCommit(
    await repository([f.tool]),
    ["accept ours"],
    undefined,
    false,
  );
  assert.equal(
    (await run(f.tool, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
    (await run(f.native, ["show", "-s", "--format=%T %P", "HEAD"])).toString(),
  );
});

test("local multi-upstream pull explicitly rejects octopus instead of discarding merge candidates", async (t) => {
  const f = await mergeFixture(
    t,
    async () => {},
    (dir) => writeFile(path.join(dir, "ours"), "ours\n"),
    (dir) => writeFile(path.join(dir, "theirs"), "theirs\n"),
  );
  await run(f.tool, ["branch", "second", "main"]);
  await run(f.tool, ["config", "branch.main.remote", "."]);
  await run(f.tool, ["config", "branch.main.merge", "refs/heads/topic"]);
  await run(f.tool, [
    "config",
    "--add",
    "branch.main.merge",
    "refs/heads/second",
  ]);
  const before = await refs(f.tool);
  await assert.rejects(
    pullCommand(
      await repository([f.tool]),
      undefined,
      [],
      { rebase: false },
      fetch,
    ),
    /octopus/,
  );
  assert.deepEqual(await refs(f.tool), before);
  assert.equal((await run(f.tool, ["status", "--porcelain"])).toString(), "");
});

test("native Git differential: default rebase and pull omit commits discarded by an upstream rewrite", async (t) => {
  for (const [action, prefetched, replacement] of [
    ["rebase", false, false],
    ["pull", false, false],
    ["explicit-pull", false, false],
    ["pull", true, false],
    ["explicit-pull", true, false],
    ["pull", true, true],
    ["explicit-pull", true, true],
  ] as const) {
    await t.test(
      `${action}, prefetch=${prefetched}, replacement=${replacement}`,
      async (t) => {
        const f = await fixture(t);
        await run(f.root, ["clone", f.url, f.native]);
        await cloneCommand(f.root, f.url, "tool", {}, fetch);
        for (const dir of [f.native, f.tool]) {
          await run(dir, ["config", "user.name", "Local Author"]);
          await run(dir, ["config", "user.email", "local@example.org"]);
          await writeFile(path.join(dir, "local"), "local addition\n");
        }
        await run(f.native, ["add", "local"]);
        await stage(await repository([f.tool]), ["local"], false, false);
        await createCommit(
          await repository([f.tool]),
          ["local addition"],
          undefined,
          false,
        );
        const [authorDate, committerDate] = (
          await run(f.tool, ["show", "-s", "--format=%aI %cI"])
        )
          .toString()
          .trim()
          .split(" ");
        await run(f.native, ["commit", "-m", "local addition"], {
          GIT_AUTHOR_DATE: authorDate,
          GIT_COMMITTER_DATE: committerDate,
        });
        const original = (await run(f.tool, ["rev-parse", "HEAD"])).toString();
        await run(f.bare, [
          "update-ref",
          "refs/heads/main",
          "refs/heads/topic",
        ]);
        if (replacement) {
          await run(f.source, ["checkout", "topic"]);
          await run(f.source, ["commit", "--allow-empty", "-m", "replacement"]);
          await run(f.bare, ["fetch", f.source, "+topic:main"]);
        }
        if (prefetched) {
          await run(f.native, ["fetch"]);
          await fetchCommand(
            await repository([f.tool]),
            undefined,
            [],
            {},
            fetch,
          );
        }
        if (action === "rebase") {
          await run(f.native, ["fetch"]);
          await fetchCommand(
            await repository([f.tool]),
            undefined,
            [],
            {},
            fetch,
          );
          await run(f.native, ["rebase"]);
          await rebaseCommand(await repository([f.tool]));
        } else {
          const explicit = action === "explicit-pull";
          await run(f.native, [
            "pull",
            "--rebase",
            ...(explicit ? ["origin", "main"] : []),
          ]);
          await pullCommand(
            await repository([f.tool]),
            explicit ? "origin" : undefined,
            explicit ? ["main"] : [],
            { rebase: true },
            fetch,
          );
        }
        assert.equal(
          (await run(f.tool, ["show", "-s", "--format=%T %P %s"])).toString(),
          (await run(f.native, ["show", "-s", "--format=%T %P %s"])).toString(),
        );
        assert.notEqual(
          (await run(f.tool, ["rev-parse", "HEAD"])).toString(),
          original,
        );
        assert.equal(
          await readFile(path.join(f.tool, "hello.txt"), "utf8"),
          "first\n",
        );
        await compareMergeState(f);
      },
    );
  }
});

test("native Git differential: rebase really replays commits and preserves authors and originally empty commits", async (t) => {
  const f = await mergeFixture(
    t,
    async () => {},
    (dir) => writeFile(path.join(dir, "local"), "local\n"),
    (dir) => writeFile(path.join(dir, "remote"), "remote\n"),
  );
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["commit", "--allow-empty", "-m", "originally empty"], {
      GIT_AUTHOR_DATE: "2025-02-03T04:05:06Z",
      GIT_COMMITTER_DATE: "2025-02-03T04:05:06Z",
    });
    await run(dir, ["config", "user.name", "New Committer"]);
    await run(dir, ["config", "user.email", "committer@example.org"]);
    await writeFile(path.join(dir, "untracked"), "keep\n");
  }
  await run(f.native, ["rebase", "topic"]);
  await rebaseCommand(await repository([f.tool]), "topic");
  const args = [
    "log",
    "--format=%T %s %an <%ae> %at %ai %cn <%ce>",
    "topic..HEAD",
  ];
  assert.equal(
    (await run(f.tool, args)).toString(),
    (await run(f.native, args)).toString(),
  );
  const onto = (await run(f.tool, ["rev-parse", "topic"])).toString().trim();
  assert.equal(
    (await run(f.tool, ["rev-parse", "HEAD~2"])).toString().trim(),
    onto,
  );
  assert.equal(
    (await run(f.tool, ["symbolic-ref", "HEAD"])).toString().trim(),
    "refs/heads/main",
  );
  await compareMergeState(f);
  assert.equal(
    await readFile(path.join(f.tool, "untracked"), "utf8"),
    "keep\n",
  );
});

test("rebase difference: no whitespace patch-ID filtering, while identical and binary trees still drop empty replay", async (t) => {
  for (const [label, base, ours, theirs, conflict] of [
    ["space and tab", "before\n", "after \n", "after\t\n", false],
    ["missing newline", "before", "after ", "after\t", false],
    [
      "literal marker-like content",
      "before",
      "\\ No newline at end of file ",
      "\\ No newline at end of file\t",
      false,
    ],
    ["vertical tab is significant", "before\n", "after\v\n", "after \n", true],
    [
      "unicode space is significant",
      "before\n",
      "after\u00a0\n",
      "after \n",
      true,
    ],
    [
      "binary",
      Buffer.from([0, 1]),
      Buffer.from([0, 2, 255]),
      Buffer.from([0, 2, 255]),
      false,
    ],
  ] as const) {
    await t.test(label, async (t) => {
      const f = await mergeFixture(
        t,
        (dir) => writeFile(path.join(dir, "equivalent"), base),
        (dir) => writeFile(path.join(dir, "equivalent"), ours),
        (dir) => writeFile(path.join(dir, "equivalent"), theirs),
      );
      if (conflict) {
        await assert.rejects(run(f.native, ["rebase", "topic"]));
        await assert.rejects(
          rebaseCommand(await repository([f.tool]), "topic"),
          /rebase conflicts/,
        );
      } else {
        await run(f.native, ["rebase", "topic"]);
        if (label !== "binary") {
          const original = (
            await run(f.tool, ["rev-parse", "HEAD"])
          ).toString();
          await assert.rejects(
            rebaseCommand(await repository([f.tool]), "topic"),
            /rebase conflicts/,
          );
          assert.ok((await run(f.tool, ["ls-files", "--unmerged"])).length > 0);
          await abortRebase(await repository([f.tool]));
          assert.equal(
            (await run(f.tool, ["rev-parse", "HEAD"])).toString(),
            original,
          );
          assert.deepEqual(
            await readFile(path.join(f.tool, "equivalent")),
            Buffer.from(ours),
          );
          return;
        }
        await rebaseCommand(await repository([f.tool]), "topic");
        assert.equal(
          (await run(f.tool, ["rev-parse", "HEAD"])).toString(),
          (await run(f.native, ["rev-parse", "HEAD"])).toString(),
        );
      }
      await compareMergeState(f);
    });
  }
});

test("native Git differential: rebase continue, manual commit, skip and abort preserve index, trees and parent chains", async (t) => {
  for (const action of ["continue", "commit", "skip", "abort"] as const) {
    await t.test(action, async (t) => {
      const f = await mergeFixture(
        t,
        (dir) => writeFile(path.join(dir, "conflict"), "base\n"),
        (dir) => writeFile(path.join(dir, "conflict"), "local\n"),
        async (dir) => {
          await writeFile(path.join(dir, "conflict"), "remote\n");
          await writeFile(
            path.join(dir, "nonconflicting"),
            "remote addition\n",
          );
        },
      );
      await assert.rejects(run(f.native, ["rebase", "topic"]));
      await assert.rejects(
        rebaseCommand(await repository([f.tool]), "topic"),
        /rebase conflicts/,
      );
      await compareMergeState(f);
      if (action === "continue" || action === "commit") {
        for (const dir of [f.native, f.tool])
          await writeFile(path.join(dir, "conflict"), "resolved\n");
        await run(f.native, ["add", "conflict"]);
        await stage(await repository([f.tool]), ["conflict"], false, false);
        if (action === "commit") {
          for (const dir of [f.native, f.tool]) {
            await run(dir, ["config", "user.name", "Manual Committer"]);
            await run(dir, ["config", "user.email", "manual@example.org"]);
          }
          await run(f.native, ["commit", "-m", "manual resolution"]);
          await createCommit(
            await repository([f.tool]),
            ["manual resolution"],
            undefined,
            false,
          );
        }
        await run(f.native, ["rebase", "--continue"], { GIT_EDITOR: "true" });
        await continueRebase(await repository([f.tool]));
      } else if (action === "skip") {
        await run(f.native, ["rebase", "--skip"]);
        await skipRebase(await repository([f.tool]));
      } else {
        await run(f.native, ["rebase", "--abort"]);
        await abortRebase(await repository([f.tool]));
      }
      await compareMergeState(f);
      const format =
        action === "commit" ? "%T %P %s %an <%ae>" : "%T %P %s %an <%ae> %at";
      assert.equal(
        (
          await run(f.tool, ["show", "-s", `--format=${format}`, "HEAD"])
        ).toString(),
        (
          await run(f.native, ["show", "-s", `--format=${format}`, "HEAD"])
        ).toString(),
      );
      if (action === "commit") {
        const nativeDate = Number(
          (await run(f.native, ["show", "-s", "--format=%at"]))
            .toString()
            .trim(),
        );
        const toolDate = Number(
          (await run(f.tool, ["show", "-s", "--format=%at"])).toString().trim(),
        );
        assert.ok(
          Math.abs(nativeDate - toolDate) < 5,
          "manual commit creates a new author date, not the original replay date",
        );
      }
      assert.equal(
        (await run(f.tool, ["symbolic-ref", "HEAD"])).toString(),
        (await run(f.native, ["symbolic-ref", "HEAD"])).toString(),
      );
    });
  }
});

test("native Git differential: rebase continue drops an empty conflict resolution and replays the next commit", async (t) => {
  const f = await mergeFixture(
    t,
    (dir) => writeFile(path.join(dir, "conflict"), "base\n"),
    (dir) => writeFile(path.join(dir, "conflict"), "local\n"),
    (dir) => writeFile(path.join(dir, "conflict"), "remote\n"),
  );
  for (const dir of [f.native, f.tool]) {
    await writeFile(path.join(dir, "later"), "later change\n");
    await run(dir, ["add", "later"]);
    await run(dir, ["commit", "-m", "later"], {
      GIT_AUTHOR_DATE: "2025-02-03T04:05:06Z",
      GIT_COMMITTER_DATE: "2025-02-03T04:05:06Z",
    });
  }
  await assert.rejects(run(f.native, ["rebase", "topic"]));
  await assert.rejects(
    rebaseCommand(await repository([f.tool]), "topic"),
    /rebase conflicts/,
  );
  for (const dir of [f.native, f.tool])
    await writeFile(path.join(dir, "conflict"), "remote\n");
  await run(f.native, ["add", "conflict"]);
  await stage(await repository([f.tool]), ["conflict"], false, false);
  await run(f.native, ["rebase", "--continue"], { GIT_EDITOR: "true" });
  await continueRebase(await repository([f.tool]));
  assert.equal(
    (
      await run(f.tool, ["show", "-s", "--format=%T %P %s %an <%ae> %at"])
    ).toString(),
    (
      await run(f.native, ["show", "-s", "--format=%T %P %s %an <%ae> %at"])
    ).toString(),
  );
  assert.equal(
    await readFile(path.join(f.tool, "later"), "utf8"),
    "later change\n",
  );
  await compareMergeState(f);
});

test("attribute-dependent rebase explicitly rejects forced binary and forced text before changing repository state", async (t) => {
  for (const [attribute, before, ours, theirs] of [
    ["-diff", "before\n", "after \n", "after\t\n"],
    ["diff", "before\0\n", "after \0\n", "after\t\0\n"],
  ] as const) {
    await t.test(attribute, async (t) => {
      const f = await mergeFixture(
        t,
        async (dir) => {
          await writeFile(
            path.join(dir, ".gitattributes"),
            `equivalent ${attribute}\n`,
          );
          await writeFile(path.join(dir, "equivalent"), before);
        },
        (dir) => writeFile(path.join(dir, "equivalent"), ours),
        (dir) => writeFile(path.join(dir, "equivalent"), theirs),
      );
      if (attribute === "-diff")
        await assert.rejects(run(f.native, ["rebase", "topic"]));
      else await run(f.native, ["rebase", "topic"]);
      const oldRefs = await refs(f.tool),
        index = await readFile(path.join(f.tool, ".git/index"));
      await assert.rejects(
        rebaseCommand(await repository([f.tool]), "topic"),
        /attribute-dependent rebase is not supported/,
      );
      assert.deepEqual(await refs(f.tool), oldRefs);
      assert.deepEqual(await readFile(path.join(f.tool, ".git/index")), index);
      assert.equal(
        await readFile(path.join(f.tool, "equivalent"), "utf8"),
        ours,
      );
      await assert.rejects(
        readFile(path.join(f.tool, ".git/golem-rebase-state")),
        { code: "ENOENT" },
      );
    });
  }
});

test("merge-commit rebase history is explicitly rejected without detaching or changing the index", async (t) => {
  const f = await mergeFixture(
    t,
    async () => {},
    (dir) => writeFile(path.join(dir, "ours"), "ours\n"),
    (dir) => writeFile(path.join(dir, "theirs"), "theirs\n"),
  );
  for (const dir of [f.native, f.tool]) {
    await run(dir, ["merge", "--no-ff", "topic", "-m", "merge"]);
    await run(dir, ["checkout", "topic"]);
    await writeFile(path.join(dir, "new-topic"), "new upstream\n");
    await run(dir, ["add", "new-topic"]);
    await run(dir, ["commit", "-m", "new topic"], {
      GIT_AUTHOR_DATE: "2025-02-03T04:05:06Z",
      GIT_COMMITTER_DATE: "2025-02-03T04:05:06Z",
    });
    await run(dir, ["checkout", "main"]);
  }
  await run(f.native, ["rebase", "topic"]);
  const oldRefs = await refs(f.tool),
    head = await readFile(path.join(f.tool, ".git/HEAD")),
    index = await readFile(path.join(f.tool, ".git/index"));
  await assert.rejects(
    rebaseCommand(await repository([f.tool]), "topic"),
    /rebasing merge commits is not supported/,
  );
  assert.deepEqual(await refs(f.tool), oldRefs);
  assert.deepEqual(await readFile(path.join(f.tool, ".git/HEAD")), head);
  assert.deepEqual(await readFile(path.join(f.tool, ".git/index")), index);
});

test("released-library boundaries: raw upload-pack and an unfinished invalid receive-pack can stall without publishing local refs", async (t) => {
  for (const mode of ["raw-fetch", "open-push"]) {
    await t.test(mode, async (t) => {
      const f = await fixture(t);
      await run(f.root, ["clone", f.url, f.tool]);
      await run(f.root, ["clone", f.url, f.native]);
      await commitBoth(f);
      const before = await refs(f.tool);
      const head = (await run(f.bare, ["rev-parse", "main"])).toString().trim();
      const packFile = path.join(f.root, "pack");
      await writeFile(
        packFile,
        await run(f.bare, ["pack-objects", "--stdout", "--all"]),
      );
      // Make a fetch publication visible if the supposedly stalled call completes.
      await run(f.tool, ["update-ref", "-d", "refs/remotes/origin/main"]);
      const pendingRefs = await refs(f.tool);
      const child = spawn(
        process.execPath,
        [
          "--experimental-strip-types",
          "cli/golem-cli/test-data/git-remote/transport-probe.ts",
          f.tool,
          mode,
          head,
          packFile,
        ],
        { stdio: "pipe" },
      );
      const output: Buffer[] = [],
        errors: Buffer[] = [];
      child.stdout.on("data", (bytes) => output.push(bytes));
      child.stderr.on("data", (bytes) => errors.push(bytes));
      const closed = new Promise((resolve) => child.once("close", resolve));
      t.after(async () => {
        child.kill();
        await closed;
      });
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(
          () =>
            reject(new Error(`probe did not start: ${Buffer.concat(errors)}`)),
          10000,
        );
        child.stdout.on("data", () => {
          if (Buffer.concat(output).toString().includes("READY")) {
            clearTimeout(timer);
            resolve();
          }
        });
        child.once("error", reject);
      });
      await new Promise((resolve) => setTimeout(resolve, 500));
      assert.equal(child.exitCode, null, Buffer.concat(errors).toString());
      assert.equal(Buffer.concat(output).toString(), "READY\n");
      child.kill();
      await closed;
      assert.deepEqual(await refs(f.tool), pendingRefs);
      assert.ok(before.length > pendingRefs.length);
      assert.equal(
        (await statusEntries(await repository([f.tool]), [])).length,
        0,
      );
    });
  }
});
