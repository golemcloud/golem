import assert from "node:assert/strict";
import fs from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import * as git from "isomorphic-git";
import {
  diff,
  effectiveCwd,
  formatStatus,
  parseIdentity,
  repository,
  stage,
  statusEntries,
} from "../src/git.ts";

async function fixture(): Promise<string> {
  const dir = await mkdtemp(path.join(os.tmpdir(), "golem-git-tool-"));
  await git.init({ fs, dir, defaultBranch: "main" });
  await git.setConfig({ fs, dir, path: "user.name", value: "Test User" });
  await git.setConfig({
    fs,
    dir,
    path: "user.email",
    value: "test@example.com",
  });
  return dir;
}

test("repeated -C directories apply sequentially", () => {
  assert.equal(
    effectiveCwd(["workspace", "src", ".."]).split("\\").join("/"),
    "/workspace",
  );
});

test("author syntax is strict", () => {
  assert.deepEqual(parseIdentity("A User <a@example.com>"), {
    name: "A User",
    email: "a@example.com",
  });
  assert.throws(() => parseIdentity("A User"), /Name <email>/);
});

test("status distinguishes index and worktree changes", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "head\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "initial",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(path.join(dir, "file.txt"), "index\n");
  await stage(repo, ["file.txt"], false, false);
  await writeFile(path.join(dir, "file.txt"), "worktree\n");

  const entries = await statusEntries(repo, []);
  assert.deepEqual(entries, [
    { path: "file.txt", index: "modified", worktree: "modified", code: "MM" },
  ]);
  assert.equal(formatStatus(entries, false), "MM file.txt\n");
});

test("diff compares index to worktree and HEAD to index independently", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "head\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "initial",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(path.join(dir, "file.txt"), "index\n");
  await stage(repo, ["file.txt"], false, false);
  await writeFile(path.join(dir, "file.txt"), "worktree\n");

  const unstaged = await diff(repo, [], [], false, false, 3);
  const staged = await diff(repo, [], [], true, false, 3);
  assert.match(unstaged.patch, /-index\n\+worktree/);
  assert.doesNotMatch(unstaged.patch, /-head/);
  assert.match(staged.patch, /-head\n\+index/);
  assert.doesNotMatch(staged.patch, /worktree/);
});
