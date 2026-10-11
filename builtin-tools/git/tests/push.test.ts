import assert from "node:assert/strict";
import fs from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import * as git from "isomorphic-git";
import { repository } from "../src/git.ts";
import { pushCommand } from "../src/push.ts";
import { remoteAdd } from "../src/remote.ts";

const zero = "0000000000000000000000000000000000000000";
const url = "https://example.org/repo.git";
const pkt = (value: string) =>
  `${(Buffer.byteLength(value) + 4).toString(16).padStart(4, "0")}${value}`;
async function fixture(t: test.TestContext) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "golem-git-push-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await git.init({ fs, dir, defaultBranch: "main" });
  const repo = await repository([dir]);
  const author = {
    name: "Push Author",
    email: "author@example.org",
    timestamp: 1,
    timezoneOffset: 0,
  };
  const first = await git.commit({ fs, ...repo, message: "first", author });
  const second = await git.commit({ fs, ...repo, message: "second", author });
  await remoteAdd(repo, "origin", url);
  await git.writeRef({
    fs,
    ...repo,
    ref: "refs/remotes/origin/main",
    value: first,
  });
  const refs = new Map([["refs/heads/main", first]]);
  let atomic = true;
  let reject = false;
  let report: string | undefined;
  let discoveries = 0;
  const requests: Buffer[] = [];
  const send: typeof fetch = async (_input, options) => {
    if (options?.method !== "POST") {
      discoveries++;
      const capabilities = `report-status delete-refs side-band-64k${atomic ? " atomic" : ""}`;
      const advertisement =
        pkt("# service=git-receive-pack\n") +
        "0000" +
        [...refs]
          .map(([ref, oid], i) =>
            pkt(`${oid} ${ref}${i ? "" : `\0${capabilities}`}\n`),
          )
          .join("") +
        "0000";
      return new Response(advertisement, {
        headers: {
          "content-type": "application/x-git-receive-pack-advertisement",
        },
      });
    }
    const bytes = Buffer.from(await new Response(options.body).arrayBuffer());
    requests.push(bytes);
    let offset = 0;
    const updates: [string, string, string][] = [];
    for (;;) {
      const length = Number.parseInt(
        bytes.subarray(offset, offset + 4).toString(),
        16,
      );
      offset += 4;
      if (!length) break;
      const line = bytes
        .subarray(offset, offset + length - 4)
        .toString()
        .split("\0")[0]!
        .trim();
      offset += length - 4;
      updates.push(line.split(" ") as [string, string, string]);
    }
    const statuses = updates.map(([old, oid, ref]) => {
      if (reject || (refs.get(ref) ?? zero) !== old)
        return pkt(`ng ${ref} server rejected\n`);
      if (oid === zero) refs.delete(ref);
      else refs.set(ref, oid);
      return pkt(`ok ${ref}\n`);
    });
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(
          Buffer.from(
            pkt(
              `\x01${report ?? pkt("unpack ok\n") + statuses.join("") + "0000"}`,
            ) + "0000",
          ),
        );
        controller.close();
      },
    });
    return new Response(body, {
      headers: { "content-type": "application/x-git-receive-pack-result" },
    });
  };
  return {
    repo,
    first,
    second,
    refs,
    requests,
    send,
    noAtomic: () => {
      atomic = false;
    },
    reject: () => {
      reject = true;
    },
    report: (value?: string) => {
      report = value;
    },
    discoveries: () => discoveries,
  };
}

test("stale and absent leases reject before transfer and never refresh tracking refs", async (t) => {
  const f = await fixture(t);
  f.refs.set("refs/heads/main", f.second);
  await assert.rejects(
    pushCommand(
      f.repo,
      "origin",
      [`${f.first}:refs/heads/main`],
      { forceWithLease: true },
      f.send,
    ),
    /stale force-with-lease/,
  );
  assert.equal(f.requests.length, 0);
  assert.equal(
    await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
    f.first,
  );
  await assert.rejects(
    pushCommand(
      f.repo,
      "origin",
      [`${f.first}:refs/heads/main`],
      { forceWithLease: true, lease: "refs/heads/main:" },
      f.send,
    ),
    /stale force-with-lease/,
  );
  assert.equal(f.requests.length, 0);
});

test("server rejection and unsupported atomic push never set upstream or advance tracking", async (t) => {
  const f = await fixture(t);
  f.noAtomic();
  await assert.rejects(
    pushCommand(
      f.repo,
      "origin",
      ["main"],
      { atomic: true, setUpstream: true },
      f.send,
    ),
    /atomic push is unsupported/,
  );
  assert.equal(f.requests.length, 0);
  f.reject();
  await assert.rejects(
    pushCommand(f.repo, "origin", ["main"], { setUpstream: true }, f.send),
    /server rejected/,
  );
  assert.equal(
    await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
    f.first,
  );
  assert.equal(
    await git.getConfig({ fs, ...f.repo, path: "branch.main.remote" }),
    undefined,
  );
});

test("successful and up-to-date pushes set upstream and explicit force remains available", async (t) => {
  const f = await fixture(t);
  const firstPush = await pushCommand(
    f.repo,
    "origin",
    ["main"],
    { setUpstream: true },
    f.send,
  );
  assert.equal(firstPush.refs[0]!.oid, f.second);
  assert.equal(f.refs.get("refs/heads/main"), f.second);
  assert.equal(
    await git.getConfig({ fs, ...f.repo, path: "branch.main.merge" }),
    "refs/heads/main",
  );
  assert.ok(
    (await pushCommand(f.repo, undefined, [], {}, f.send)).refs[0]!.upToDate,
  );
  await pushCommand(
    f.repo,
    "origin",
    [`${f.first}:refs/heads/main`],
    { force: true },
    f.send,
  );
  assert.equal(f.refs.get("refs/heads/main"), f.first);
});

test("released receive-pack report parsing rejects missing status but tolerates malformed trailing records", async (t) => {
  const success = pkt("unpack ok\n") + pkt("ok refs/heads/main\n");
  for (const [index, report] of [
    success + "00",
    success + "0010abc",
    success,
    success + "0001",
    success + "0002",
    success + pkt("ok refs/heads/main\n") + "0000",
    pkt("unpack ok\n") + pkt("what refs/heads/main\n") + "0000",
    success + "0000" + pkt("ok refs/heads/main\n"),
  ].entries()) {
    await t.test(`report ${index}`, async () => {
      const f = await fixture(t);
      f.report(report);
      if (index !== 6 && index !== 1) {
        const result = await pushCommand(
          f.repo,
          "origin",
          ["main"],
          { setUpstream: true },
          f.send,
        );
        assert.ok(result.refs[0]!.ok);
        assert.equal(
          await git.resolveRef({
            fs,
            ...f.repo,
            ref: "refs/remotes/origin/main",
          }),
          f.second,
        );
        assert.equal(
          await git.getConfig({ fs, ...f.repo, path: "branch.main.merge" }),
          "refs/heads/main",
        );
        return;
      }
      await assert.rejects(
        pushCommand(f.repo, "origin", ["main"], { setUpstream: true }, f.send),
      );
      assert.equal(
        await git.resolveRef({
          fs,
          ...f.repo,
          ref: "refs/remotes/origin/main",
        }),
        f.first,
      );
      assert.equal(
        await git.getConfig({ fs, ...f.repo, path: "branch.main.remote" }),
        undefined,
      );
      f.report();
      const result = await pushCommand(
        f.repo,
        "origin",
        ["main"],
        { setUpstream: true },
        f.send,
      );
      assert.ok(result.refs[0]!.ok);
      assert.equal(
        await git.resolveRef({
          fs,
          ...f.repo,
          ref: "refs/remotes/origin/main",
        }),
        f.second,
      );
    });
  }
  const f = await fixture(t);
  f.report(pkt("unpack ok\0invalid\n"));
  await assert.rejects(
    pushCommand(f.repo, "origin", ["main"], {}, f.send),
    /not updated/,
  );
});

test("push isolates upstream tracking writes, honors custom/excluded mappings and records the published reflog", async (t) => {
  for (const remote of ["other", url]) {
    await t.test(remote, async (t) => {
      const f = await fixture(t);
      await remoteAdd(f.repo, "other", url);
      await git.setConfig({
        fs,
        ...f.repo,
        path: "remote.other.fetch",
        value: "+refs/heads/*:refs/remotes/custom/*",
      });
      await pushCommand(f.repo, remote, ["main"], {}, f.send);
      assert.equal(
        await git.resolveRef({
          fs,
          ...f.repo,
          ref: "refs/remotes/origin/main",
        }),
        f.first,
      );
      await assert.rejects(
        git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/other/main" }),
      );
      if (remote === "other") {
        assert.equal(
          await git.resolveRef({
            fs,
            ...f.repo,
            ref: "refs/remotes/custom/main",
          }),
          f.second,
        );
        assert.match(
          await readFile(
            path.join(f.repo.gitdir, "logs/refs/remotes/custom/main"),
            "utf8",
          ),
          new RegExp(`${zero} ${f.second} `),
        );
      } else
        await assert.rejects(
          git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/custom/main" }),
        );
      await pushCommand(
        f.repo,
        remote,
        ["refs/heads/main"],
        { delete: true },
        f.send,
      );
      assert.equal(
        await git.resolveRef({
          fs,
          ...f.repo,
          ref: "refs/remotes/origin/main",
        }),
        f.first,
      );
    });
  }
  const f = await fixture(t);
  await git.setConfig({
    fs,
    ...f.repo,
    path: "remote.origin.fetch",
    value: "^refs/heads/main",
    append: true,
  });
  await pushCommand(f.repo, "origin", ["main"], {}, f.send);
  assert.equal(
    await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
    f.first,
  );
});

test("unexpected successful status refs never publish tracking or upstream", async (t) => {
  const f = await fixture(t);
  f.report(
    pkt("unpack ok\n") +
      pkt("ok refs/heads/main\n") +
      pkt("ok refs/heads/unexpected\n") +
      "0000",
  );
  await assert.rejects(
    pushCommand(f.repo, "origin", ["main"], { setUpstream: true }, f.send),
    /invalid or incomplete/,
  );
  assert.equal(
    await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
    f.first,
  );
  assert.equal(
    await git.getConfig({ fs, ...f.repo, path: "branch.main.remote" }),
    undefined,
  );
  await assert.rejects(
    readFile(path.join(f.repo.gitdir, "logs/refs/remotes/origin/main")),
  );
});

test("short destination ambiguity is non-destructive; existing tags require force for branch and OID sources", async (t) => {
  const f = await fixture(t);
  f.refs.set("refs/heads/release", f.first);
  f.refs.set("refs/tags/release", f.first);
  await assert.rejects(
    pushCommand(f.repo, "origin", ["release"], { delete: true }, f.send),
    /matches more than one remote/,
  );
  assert.equal(f.requests.length, 0);
  await pushCommand(
    f.repo,
    "origin",
    ["refs/heads/release"],
    { delete: true },
    f.send,
  );
  assert.equal(f.refs.get("refs/tags/release"), f.first);
  for (const source of ["main", f.second]) {
    await assert.rejects(
      pushCommand(
        f.repo,
        "origin",
        [`${source}:refs/tags/release`],
        {},
        f.send,
      ),
      /tag requires force/,
    );
  }
  await assert.rejects(
    pushCommand(f.repo, "origin", ["main:release"], {}, f.send),
    /tag requires force/,
  );
  const posts = f.requests.length;
  await assert.rejects(
    pushCommand(
      f.repo,
      "origin",
      ["main:release"],
      { forceWithLease: true, lease: `refs/tags/release:${f.second}` },
      f.send,
    ),
    /stale force-with-lease/,
  );
  assert.equal(f.requests.length, posts);
  await pushCommand(
    f.repo,
    "origin",
    ["main:release"],
    { forceWithLease: true, lease: `refs/tags/release:${f.first}` },
    f.send,
  );
  assert.equal(f.refs.get("refs/tags/release"), f.second);
  assert.equal(f.refs.has("refs/heads/release"), false);
  await pushCommand(
    f.repo,
    "origin",
    [`${f.first}:refs/tags/release`],
    { force: true },
    f.send,
  );
  assert.equal(f.refs.get("refs/tags/release"), f.first);
});

test("atomic and multi-ref push reject before any HTTP discovery or transfer", async (t) => {
  const f = await fixture(t);
  for (const [specs, options] of [
    [["main"], { atomic: true }],
    [["main:one", "main:two"], {}],
  ] as const)
    await assert.rejects(
      pushCommand(f.repo, "origin", [...specs], options, f.send),
      /push is unsupported/,
    );
  assert.equal(f.discoveries(), 0);
  assert.equal(f.requests.length, 0);
});

test("qualified destinations never expand to a nested lookalike or bypass an absent-reference lease", async (t) => {
  for (const deleting of [true, false]) {
    await t.test(
      deleting ? "delete" : "create with absent lease",
      async (t) => {
        const f = await fixture(t);
        const nested = "refs/heads/refs/heads/release";
        f.refs.set(nested, f.first);
        await assert.rejects(
          pushCommand(
            f.repo,
            "origin",
            deleting ? ["refs/heads/release"] : ["main:refs/heads/release"],
            deleting
              ? { delete: true }
              : {
                  forceWithLease: true,
                  lease: "refs/heads/release:",
                  setUpstream: true,
                },
            f.send,
          ),
        );
        assert.equal(f.requests.length, 0);
        assert.equal(f.refs.get(nested), f.first);
        assert.equal(f.refs.has("refs/heads/release"), false);
        assert.equal(
          await git.resolveRef({
            fs,
            ...f.repo,
            ref: "refs/remotes/origin/main",
          }),
          f.first,
        );
        assert.equal(
          await git.getConfig({ fs, ...f.repo, path: "branch.main.remote" }),
          undefined,
        );
      },
    );
  }
});
