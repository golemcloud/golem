import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { deflateSync } from "node:zlib";
import * as git from "isomorphic-git";
import { repository } from "../src/git.ts";
import { configure, remoteAdd } from "../src/remote.ts";
import { cloneCommand, fetchCommand } from "../src/transfer.ts";

const url = "https://example.org/project.git";
const person = {
  name: "Author",
  email: "author@example.org",
  timestamp: 1,
  timezoneOffset: 0,
};
const packet = (bytes: Buffer | string) => {
  const content = Buffer.from(bytes);
  return Buffer.concat([
    Buffer.from((content.length + 4).toString(16).padStart(4, "0")),
    content,
  ]);
};

async function fixture(t: test.TestContext) {
  const root = await mkdtemp(path.join(os.tmpdir(), "golem-git-transfer-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const server = path.join(root, "server");
  const client = path.join(root, "client");
  await git.init({ fs, dir: server, defaultBranch: "main" });
  await git.init({ fs, dir: client, defaultBranch: "main" });
  const blob = await git.writeBlob({
    fs,
    dir: server,
    blob: new TextEncoder().encode("hello\n"),
  });
  const tree = await git.writeTree({
    fs,
    dir: server,
    tree: [{ mode: "100644", path: "hello.txt", oid: blob, type: "blob" }],
  });
  const first = await git.writeCommit({
    fs,
    dir: server,
    commit: {
      tree,
      parent: [],
      author: person,
      committer: person,
      message: "first\n",
    },
  });
  const second = await git.writeCommit({
    fs,
    dir: server,
    commit: {
      tree,
      parent: [first],
      author: person,
      committer: person,
      message: "second\n",
    },
  });
  const refs = new Map([
    ["HEAD", second],
    ["refs/heads/main", second],
    ["refs/heads/topic", first],
    ["refs/tags/v1", first],
  ]);
  let pack = Buffer.from(
    (
      await git.packObjects({
        fs,
        dir: server,
        oids: [first, second, tree, blob],
        write: false,
      })
    ).packfile!,
  );
  let sideband = true;
  const send: typeof fetch = async (_input, options) => {
    if (options?.method !== "POST") {
      const advertisement = [
        packet("# service=git-upload-pack\n"),
        Buffer.from("0000"),
      ];
      for (const [index, [ref, oid]] of [...refs].entries()) {
        advertisement.push(
          packet(
            `${oid} ${ref}${index === 0 ? `\0multi_ack_detailed ofs-delta symref=HEAD:refs/heads/main${sideband ? " side-band-64k" : ""}` : ""}\n`,
          ),
        );
      }
      advertisement.push(Buffer.from("0000"));
      return new Response(Buffer.concat(advertisement), {
        headers: {
          "content-type": "application/x-git-upload-pack-advertisement",
        },
      });
    }
    const contents = Buffer.concat([
      packet("NAK\n"),
      ...(sideband
        ? [packet(Buffer.concat([Buffer.from([1]), pack])), Buffer.from("0000")]
        : [pack]),
    ]);
    return new Response(contents, {
      headers: { "content-type": "application/x-git-upload-pack-result" },
    });
  };
  const repo = await repository([client]);
  await remoteAdd(repo, "origin", url);
  return {
    root,
    server,
    repo,
    blob,
    tree,
    first,
    second,
    refs,
    send,
    setPack: (value: Buffer) => {
      pack = value;
    },
    corrupt: () => {
      pack = Buffer.from(pack);
      pack[pack.length - 1] ^= 1;
    },
    sideband: () => {
      sideband = true;
    },
  };
}

test("fetch publishes multiple mapped refs after sideband transfer and writes merge candidates", async (t) => {
  const { repo, first, second, send } = await fixture(t);
  await configure(repo, "branch.main.remote", "origin", {
    add: false,
    getAll: false,
    unset: false,
    unsetAll: false,
  });
  await configure(repo, "branch.main.merge", "refs/heads/main", {
    add: false,
    getAll: false,
    unset: false,
    unsetAll: false,
  });
  await fetchCommand(repo, undefined, [], {}, send);
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/main" }),
    second,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/topic" }),
    first,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/tags/v1" }),
    first,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    `${second}\t\tbranch 'main' of ${url}\n${first}\tnot-for-merge\tbranch 'topic' of ${url}\n${first}\tnot-for-merge\ttag 'v1' of ${url}\n`,
  );
  assert.equal(
    new TextDecoder().decode(
      (await git.readBlob({ fs, ...repo, oid: second, filepath: "hello.txt" }))
        .blob,
    ),
    "hello\n",
  );
  await assert.rejects(readFile(path.join(repo.gitdir, "golem-fetch-state")), {
    code: "ENOENT",
  });
});

test("sideband transfer obeys explicit and negative refspecs and FETCH_HEAD append/no-write", async (t) => {
  const { repo, first, second, send, sideband } = await fixture(t);
  sideband();
  await fetchCommand(
    repo,
    "origin",
    ["main:refs/remotes/custom/main"],
    { tags: false },
    send,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/custom/main" }),
    second,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/main" }),
    second,
  );
  await assert.rejects(
    git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/topic" }),
  );
  const initial = await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8");
  await fetchCommand(
    repo,
    "origin",
    ["topic"],
    { append: true, tags: false },
    send,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    initial + `${first}\t\tbranch 'topic' of ${url}\n`,
  );
  await fetchCommand(
    repo,
    "origin",
    ["+refs/heads/*:refs/remotes/filtered/*", "^refs/heads/topic"],
    { writeFetchHead: false, tags: false },
    send,
  );
  await assert.rejects(
    git.resolveRef({ fs, ...repo, ref: "refs/remotes/filtered/topic" }),
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/filtered/main" }),
    second,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    initial + `${first}\t\tbranch 'topic' of ${url}\n`,
  );
});

test("a corrupt pack trailer never publishes refs, FETCH_HEAD, or shallow metadata", async (t) => {
  const { repo, send, corrupt } = await fixture(t);
  await writeFile(path.join(repo.gitdir, "FETCH_HEAD"), "previous\n");
  corrupt();
  await assert.rejects(
    fetchCommand(repo, "origin", [], {}, send),
    /Packfile payload corrupted/,
  );
  assert.deepEqual(
    await git.listBranches({ fs, ...repo, remote: "origin" }),
    [],
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    "previous\n",
  );
  await assert.rejects(readFile(path.join(repo.gitdir, "shallow")), {
    code: "ENOENT",
  });
});

test("pruning affects mapped refs, not unrelated local tags", async (t) => {
  const { repo, first, send, refs } = await fixture(t);
  await fetchCommand(repo, "origin", [], {}, send);
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/tags/local-only",
    value: first,
  });
  refs.delete("refs/heads/topic");
  refs.delete("refs/tags/v1");
  await fetchCommand(repo, "origin", [], { prune: true }, send);
  await assert.rejects(
    git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/topic" }),
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/tags/local-only" }),
    first,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/tags/v1" }),
    first,
  );
});

test("clone configures its selected branch and origin, and safe destinations never get overwritten", async (t) => {
  const { root, first, send } = await fixture(t);
  await cloneCommand(
    root,
    url,
    "clone",
    { branch: "topic", origin: "upstream", singleBranch: true, tags: false },
    send,
  );
  const repo = await repository([path.join(root, "clone")]);
  assert.equal(await git.resolveRef({ fs, ...repo, ref: "HEAD" }), first);
  assert.equal(await git.currentBranch({ fs, ...repo }), "topic");
  assert.equal(
    await git.getConfig({ fs, ...repo, path: "branch.topic.remote" }),
    "upstream",
  );
  assert.equal(
    await git.getConfig({ fs, ...repo, path: "branch.topic.merge" }),
    "refs/heads/topic",
  );
  assert.equal(
    await git.getConfig({ fs, ...repo, path: "remote.upstream.fetch" }),
    "+refs/heads/topic:refs/remotes/upstream/topic",
  );
  assert.deepEqual(
    await git.listBranches({ fs, ...repo, remote: "upstream" }),
    ["topic"],
  );
  assert.equal(
    await readFile(path.join(repo.dir, "hello.txt"), "utf8"),
    "hello\n",
  );
  await assert.rejects(readFile(path.join(repo.gitdir, "FETCH_HEAD")), {
    code: "ENOENT",
  });
  await assert.rejects(
    cloneCommand(root, url, "clone", {}, send),
    /empty regular directory/,
  );
  assert.equal(
    await readFile(path.join(repo.dir, "hello.txt"), "utf8"),
    "hello\n",
  );
});

test("repository opening finishes an interrupted publication before reading refs", async (t) => {
  const { repo, first } = await fixture(t);
  await writeFile(
    path.join(repo.gitdir, "golem-fetch-state"),
    JSON.stringify({
      phase: "writing",
      logs: [],
      writes: [
        ["refs/remotes/origin/main", `${first}\n`],
        ["FETCH_HEAD", `${first}\t\tbranch 'main' of ${url}\n`],
      ],
      deletions: [],
    }),
  );
  const reopened = await repository([repo.dir]);
  assert.equal(
    await git.resolveRef({ fs, ...reopened, ref: "refs/remotes/origin/main" }),
    first,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    `${first}\t\tbranch 'main' of ${url}\n`,
  );
  await assert.rejects(readFile(path.join(repo.gitdir, "golem-fetch-state")), {
    code: "ENOENT",
  });
});

test("pruning permits both ref namespace transitions and preserves unrelated packed refs", async (t) => {
  const { repo, first, second, send, refs } = await fixture(t);
  await fetchCommand(repo, "origin", [], {}, send);
  await writeFile(
    path.join(repo.gitdir, "packed-refs"),
    `# pack-refs with: peeled\n${first} refs/remotes/origin/topic\n${second} refs/tags/private\n`,
  );
  refs.delete("refs/heads/topic");
  refs.set("refs/heads/topic/subtopic", first);
  await fetchCommand(repo, "origin", [], { prune: true }, send);
  assert.equal(
    await git.resolveRef({
      fs,
      ...repo,
      ref: "refs/remotes/origin/topic/subtopic",
    }),
    first,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/tags/private" }),
    second,
  );
  refs.delete("refs/heads/topic/subtopic");
  refs.set("refs/heads/topic", second);
  await fetchCommand(repo, "origin", [], { prune: true }, send);
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/topic" }),
    second,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/tags/private" }),
    second,
  );
});

test("publication recovery separates deletion from creation across interrupted namespace changes", async (t) => {
  const { repo, first, second } = await fixture(t);
  const ref = "refs/remotes/origin/topic";
  await mkdir(path.dirname(path.join(repo.gitdir, ref)), { recursive: true });
  await writeFile(path.join(repo.gitdir, ref), `${first}\n`);
  const packed = `# pack-refs with: peeled\n${second} refs/tags/private\n`;
  await writeFile(
    path.join(repo.gitdir, "packed-refs"),
    `${packed}${first} ${ref}\n`,
  );
  await writeFile(
    path.join(repo.gitdir, "golem-fetch-state"),
    JSON.stringify({
      phase: "deleting",
      logs: [],
      writes: [[`${ref}/subtopic`, `${second}\n`]],
      deletions: [ref],
      packedRefs: packed,
    }),
  );
  await repository([repo.dir]);
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: `${ref}/subtopic` }),
    second,
  );
  assert.equal(
    await readFile(path.join(repo.gitdir, "packed-refs"), "utf8"),
    packed,
  );
  // The terminal journal removal may be interrupted after the descendant exists.
  await writeFile(
    path.join(repo.gitdir, "golem-fetch-state"),
    JSON.stringify({
      phase: "writing",
      logs: [],
      writes: [[`${ref}/subtopic`, `${second}\n`]],
      deletions: [ref],
      packedRefs: packed,
    }),
  );
  await repository([repo.dir]);
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: `${ref}/subtopic` }),
    second,
  );
});

test("negative tag selections and upstream merge refs apply to augmented fetch requests", async (t) => {
  const { repo, first, second, send } = await fixture(t);
  await git.setConfig({
    fs,
    ...repo,
    path: "remote.origin.fetch",
    value: "+refs/heads/topic:refs/remotes/origin/topic",
  });
  await git.setConfig({
    fs,
    ...repo,
    path: "branch.main.remote",
    value: "origin",
  });
  await git.setConfig({
    fs,
    ...repo,
    path: "branch.main.merge",
    value: "refs/heads/main",
  });
  await fetchCommand(repo, "origin", [], {}, send);
  assert.match(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    new RegExp(`^${second}\\t\\tbranch 'main'`),
  );
  await git.deleteRef({ fs, ...repo, ref: "refs/tags/v1" });
  await fetchCommand(
    repo,
    "origin",
    ["topic", "^refs/tags/v1"],
    { tags: true },
    send,
  );
  await assert.rejects(git.resolveRef({ fs, ...repo, ref: "refs/tags/v1" }));
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/remotes/origin/topic" }),
    first,
  );
  await fetchCommand(repo, url, [], { tags: false }, send);
  assert.match(
    await readFile(path.join(repo.gitdir, "FETCH_HEAD"), "utf8"),
    new RegExp(`^${second}\\t\\tref 'HEAD'`),
  );
});

test("a rejected live source mapping protects its destination against an absent mapping", async (t) => {
  const { repo, first, second, send } = await fixture(t);
  await fetchCommand(repo, "origin", [], {}, send);
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/heads/retained",
    value: second,
  });
  await assert.rejects(
    fetchCommand(
      repo,
      "origin",
      [
        "refs/heads/topic:refs/heads/retained",
        "refs/heads/missing/*:refs/heads/*",
      ],
      { prune: true, tags: false },
      send,
    ),
    /non-fast-forward/,
  );
  assert.equal(
    await git.resolveRef({ fs, ...repo, ref: "refs/heads/retained" }),
    second,
  );
  assert.notEqual(first, second);
});

test("force never permits a non-commit local branch and invalid graph types never publish", async (t) => {
  const f = await fixture(t);
  const tag = await git.writeTag({
    fs,
    dir: f.server,
    tag: {
      object: f.first,
      type: "commit",
      tag: "annotated",
      tagger: person,
      message: "release\n",
    },
  });
  const malformed = await git.writeCommit({
    fs,
    dir: f.server,
    commit: {
      tree: f.blob,
      parent: [],
      author: person,
      committer: person,
      message: "wrong tree\n",
    },
  });
  f.setPack(
    Buffer.from(
      (
        await git.packObjects({
          fs,
          dir: f.server,
          oids: [tag, malformed, f.first, f.second, f.tree, f.blob],
          write: false,
        })
      ).packfile!,
    ),
  );
  f.refs.set("refs/tags/annotated", tag);
  await assert.rejects(
    fetchCommand(
      f.repo,
      "origin",
      ["+refs/tags/annotated:refs/heads/new"],
      { force: true },
      f.send,
    ),
    /non-commit branch/,
  );
  await assert.rejects(
    git.resolveRef({ fs, ...f.repo, ref: "refs/heads/new" }),
  );
  f.refs.set("refs/heads/broken", malformed);
  await assert.rejects(
    fetchCommand(f.repo, "origin", ["broken"], {}, f.send),
    /expected tree, got blob/,
  );
  await assert.rejects(
    git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/broken" }),
  );
  const shallowBlob: typeof fetch = async (input, options) => {
    const response = await f.send(input, options);
    if (options?.method !== "POST") return response;
    return new Response(
      Buffer.concat([
        packet(`shallow ${f.blob}\n`),
        Buffer.from(await response.arrayBuffer()),
      ]),
      { headers: response.headers },
    );
  };
  // The released fetch API discards unsolicited shallow lines without a depth request.
  await fetchCommand(f.repo, "origin", ["main"], {}, shallowBlob);
  await assert.rejects(readFile(path.join(f.repo.gitdir, "shallow")), {
    code: "ENOENT",
  });
});

function pack(records: Buffer[], count = records.length): Buffer {
  const header = Buffer.alloc(12);
  header.write("PACK");
  header.writeUInt32BE(2, 4);
  header.writeUInt32BE(count, 8);
  const contents = Buffer.concat([header, ...records]);
  return Buffer.concat([
    contents,
    createHash("sha1").update(contents).digest(),
  ]);
}

function record(
  type: number,
  contents: Buffer,
  reference = Buffer.alloc(0),
): Buffer {
  let size = Math.floor(contents.length / 16);
  const header = [(type << 4) | (contents.length & 15) | (size ? 128 : 0)];
  while (size) {
    const byte = size & 127;
    size = Math.floor(size / 128);
    header.push(byte | (size ? 128 : 0));
  }
  return Buffer.concat([Buffer.from(header), reference, deflateSync(contents)]);
}

test("valid forward deltas transfer and malformed packs with valid checksums never publish", async (t) => {
  const f = await fixture(t);
  const records: Buffer[] = [];
  for (const [oid, type] of [
    [f.first, 1],
    [f.second, 1],
    [f.tree, 2],
  ] as const) {
    const object = await git.readObject({
      fs,
      dir: f.server,
      oid,
      format: "content",
    });
    records.push(record(type, Buffer.from(object.object as Uint8Array)));
  }
  const base = Buffer.from("hello");
  const baseOid = createHash("sha1")
    .update(Buffer.concat([Buffer.from("blob 5\0"), base]))
    .digest();
  const delta = Buffer.from([5, 6, 0x90, 5, 1, 10]);
  f.setPack(pack([...records, record(7, delta, baseOid), record(3, base)]));
  await fetchCommand(f.repo, "origin", [], {}, f.send);
  assert.equal(
    Buffer.from(
      (
        await git.readBlob({
          fs,
          ...f.repo,
          oid: f.second,
          filepath: "hello.txt",
        })
      ).blob,
    ).toString(),
    "hello\n",
  );
  const before = await readFile(path.join(f.repo.gitdir, "FETCH_HEAD"), "utf8");
  for (const [index, bad] of [
    pack([...records, record(7, delta, Buffer.alloc(20, 42))]),
    pack([
      ...records,
      record(7, Buffer.from([5, 6, 0x90, 7]), baseOid),
      record(3, base),
    ]),
    pack([...records, record(3, base)], 9),
    pack([...records, Buffer.from([0x36, 0x78])]),
    pack([...records, record(3, base), Buffer.from([1])], 4),
  ].entries()) {
    await t.test(`malformed pack ${index}`, async () => {
      const fresh = await fixture(t);
      await writeFile(path.join(fresh.repo.gitdir, "FETCH_HEAD"), before);
      fresh.setPack(bad);
      await assert.rejects(
        fetchCommand(fresh.repo, "origin", [], {}, fresh.send),
      );
      assert.equal(
        await readFile(path.join(fresh.repo.gitdir, "FETCH_HEAD"), "utf8"),
        before,
      );
      await assert.rejects(
        git.resolveRef({ fs, ...fresh.repo, ref: "refs/remotes/origin/main" }),
      );
    });
  }
});
