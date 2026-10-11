import assert from "node:assert/strict";
import fs from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
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
  let holdOpen = false;
  let disposed = 0;
  const requests: Buffer[] = [];
  const send: typeof fetch = async (_input, options) => {
    if (options?.method !== "POST") {
      const capabilities = `report-status delete-refs${atomic ? " atomic" : ""}`;
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
            report ?? pkt("unpack ok\n") + statuses.join("") + "0000",
          ),
        );
        if (!holdOpen) controller.close();
      },
      cancel() {
        disposed++;
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
    report: (value?: string, open = false) => {
      report = value;
      holdOpen = open;
    },
    disposed: () => disposed,
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
    /does not support atomic/,
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

test("invalid receive-pack reports never publish tracking or upstream and leave the repository usable", async (t) => {
  const success = pkt("unpack ok\n") + pkt("ok refs/heads/main\n");
  for (const report of [
    success + "00",
    success + "0010abc",
    success,
    success + "0001",
    success + "0002",
    success + pkt("ok refs/heads/main\n") + "0000",
    pkt("unpack ok\n") + pkt("what refs/heads/main\n") + "0000",
    success + "0000" + pkt("ok refs/heads/main\n"),
  ]) {
    const f = await fixture(t);
    f.report(report);
    await assert.rejects(
      pushCommand(f.repo, "origin", ["main"], { setUpstream: true }, f.send),
    );
    assert.equal(
      await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
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
      await git.resolveRef({ fs, ...f.repo, ref: "refs/remotes/origin/main" }),
      f.second,
    );
  }
  const f = await fixture(t);
  f.report(pkt("unpack ok\0invalid\n"), true);
  await assert.rejects(
    pushCommand(f.repo, "origin", ["main"], {}, f.send),
    /Invalid receive-pack/,
  );
  assert.equal(
    f.disposed(),
    1,
    "invalid report must dispose the still-open response body",
  );
});
