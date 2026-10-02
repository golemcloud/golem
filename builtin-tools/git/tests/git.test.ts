import assert from "node:assert/strict";
import fs from "node:fs";
import {
  chmod,
  lstat,
  mkdtemp,
  readlink,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import * as git from "isomorphic-git";
import {
  MAX_DIFF_BYTES,
  assertRefSupportedModes,
  branchCommand,
  checkoutCommand,
  commitMessage,
  createCommit,
  diff,
  effectiveCwd,
  formatLog,
  formatStatus,
  limitedDiffOutput,
  localConfig,
  parseIdentity,
  repository,
  restoreIndexPaths,
  stage,
  statusEntries,
  validatedCwd,
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

test("each repeated -C directory must exist before the next is applied", async (t) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "golem-git-tool-cwd-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await assert.rejects(
    validatedCwd([path.join(dir, "missing"), ".."]),
    /does not exist/,
  );
  const file = path.join(dir, "file");
  await writeFile(file, "not a directory");
  await assert.rejects(validatedCwd([file]), /not a directory/);
});

test("author syntax is strict", () => {
  assert.deepEqual(parseIdentity("A User <a@example.com>"), {
    name: "A User",
    email: "a@example.com",
  });
  assert.throws(() => parseIdentity("A User"), /Name <email>/);
  assert.throws(
    () => parseIdentity("Alice\0Injected <alice@example.com>"),
    /Name <email>/,
  );
});

test("commit messages use paragraphs and reject only an empty message", () => {
  assert.equal(commitMessage(["subject", "body"]), "subject\n\nbody");
  assert.equal(commitMessage(["subject", ""]), "subject\n\n");
  assert.throws(() => commitMessage([]), /must not be empty/);
  assert.throws(() => commitMessage(["", "  "]), /must not be empty/);
});

test("local config is narrow and commit keeps author and committer separate", async (t) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "golem-git-tool-commit-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await git.init({ fs, dir, defaultBranch: "main" });
  const repo = await repository([dir]);

  await assert.rejects(localConfig(repo, "user.name", undefined), /not set/);
  await assert.rejects(
    localConfig(repo, "core.editor", "false"),
    /only user.name and user.email/,
  );
  assert.equal(
    await localConfig(repo, "user.name", "Committer #1; O'Brien"),
    "Committer #1; O'Brien",
  );
  assert.equal(
    await localConfig(repo, "user.email", "committer@example.com"),
    "committer@example.com",
  );

  await writeFile(path.join(dir, "file.txt"), "content\n");
  await stage(repo, ["file.txt"], false, false);
  const oid = await createCommit(
    repo,
    ["subject", "body"],
    "Author <author@example.com>",
    false,
  );
  const { commit } = await git.readCommit({ fs, dir, oid });
  assert.equal(commit.message, "subject\n\nbody\n");
  assert.deepEqual(
    { name: commit.author.name, email: commit.author.email },
    { name: "Author", email: "author@example.com" },
  );
  assert.deepEqual(
    { name: commit.committer.name, email: commit.committer.email },
    { name: "Committer #1; O'Brien", email: "committer@example.com" },
  );

  await assert.rejects(createCommit(repo, ["unchanged"], undefined, false));
  const emptyOid = await createCommit(repo, ["empty"], undefined, true);
  const empty = await git.readCommit({ fs, dir, oid: emptyOid });
  assert.deepEqual(empty.commit.parent, [oid]);
  assert.equal(empty.commit.tree, commit.tree);
});

test("local config rejects lossy values without modifying the file", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const repo = await repository([dir]);
  const configPath = path.join(dir, ".git/config");
  const before = await fs.promises.readFile(configPath);

  for (const value of [
    "Alice\n[core]\n\tbare = true",
    'Alice "Example" Smith',
    " Alice",
    "Alice ",
    "Alice\0Admin",
    "Alice#\\",
    "Alice;\\",
    "Alice\u2028Admin",
    "Alice\u2029Admin",
  ]) {
    await assert.rejects(
      localConfig(repo, "user.name", value),
      /cannot be represented safely/,
    );
    assert.deepEqual(await fs.promises.readFile(configPath), before);
  }
  assert.equal(
    await git.getConfig({ fs, dir, path: "core.bare" }),
    false,
  );
});

test("log formatting supports full and oneline output", () => {
  const entries = [
    {
      oid: "0123456789abcdef0123456789abcdef01234567",
      message: "subject\n\nbody\n",
      authorName: "Test User",
      authorEmail: "test@example.com",
      timestamp: 1,
    },
  ];
  assert.equal(formatLog(entries, true), "0123456 subject\n");
  assert.equal(
    formatLog(entries, false),
    "commit 0123456789abcdef0123456789abcdef01234567\n" +
      "Author: Test User <test@example.com>\n\n" +
      "    subject\n    \n    body\n",
  );
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

test("untracked status uses Git porcelain codes and NUL termination", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "new file.txt"), "new\n");
  const entries = await statusEntries(await repository([dir]), []);
  assert.deepEqual(entries, [
    {
      path: "new file.txt",
      index: "unmodified",
      worktree: "untracked",
      code: "??",
    },
  ]);
  assert.equal(formatStatus(entries, false), '?? "new file.txt"\n');
  assert.equal(formatStatus(entries, true), "?? new file.txt\0");
});

test("staged deletion followed by recreation uses separate porcelain records", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "old\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await createCommit(repo, ["initial"], undefined, false);
  await rm(path.join(dir, "file.txt"));
  await stage(repo, ["file.txt"], false, false);
  await writeFile(path.join(dir, "file.txt"), "new\n");

  const entries = await statusEntries(repo, []);
  assert.deepEqual(entries, [
    {
      path: "file.txt",
      index: "deleted",
      worktree: "unmodified",
      code: "D ",
    },
    {
      path: "file.txt",
      index: "unmodified",
      worktree: "untracked",
      code: "??",
    },
  ]);
  assert.equal(formatStatus(entries, false), "D  file.txt\n?? file.txt\n");
});

test("porcelain-v1 status quotes unusual paths unless NUL-terminated", () => {
  const entries = [
    {
      path: 'line\nbreak-"-\\-é.txt',
      index: "unmodified",
      worktree: "untracked",
      code: "??",
    },
  ];

  assert.equal(
    formatStatus(entries, false),
    '?? "line\\nbreak-\\"-\\\\-\\303\\251.txt"\n',
  );
  assert.equal(formatStatus(entries, true), `?? ${entries[0].path}\0`);
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

test("diff excludes untracked files", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "tracked.txt"), "tracked\n");
  const repo = await repository([dir]);
  await stage(repo, ["tracked.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "tracked",
    author: { name: "Test User", email: "test@example.com" },
  });

  await writeFile(path.join(dir, "untracked.txt"), "untracked\n");
  assert.deepEqual(await diff(repo, [], [], false, false, 3), {
    patch: "",
    stat: "",
    paths: [],
    hasChanges: false,
  });

  await git.remove({ fs, dir, filepath: "tracked.txt" });
  assert.deepEqual(await diff(repo, [], [], false, false, 3), {
    patch: "",
    stat: "",
    paths: [],
    hasChanges: false,
  });
});

test("added and deleted file patches use Git metadata and null paths", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const repo = await repository([dir]);
  await writeFile(path.join(dir, "content.txt"), "content\n");
  await writeFile(path.join(dir, "empty.txt"), "");
  await stage(repo, [], true, false);

  const added = await diff(repo, [], [], true, false, 3);
  assert.match(
    added.patch,
    /diff --git a\/content\.txt b\/content\.txt\nnew file mode 100644\n--- \/dev\/null\n\+\+\+ b\/content\.txt/,
  );
  assert.match(
    added.patch,
    /diff --git a\/empty\.txt b\/empty\.txt\nnew file mode 100644\n--- \/dev\/null\n\+\+\+ b\/empty\.txt/,
  );
  assert.doesNotMatch(added.patch, /^=+$/m);
  await git.commit({
    fs,
    dir,
    message: "add files",
    author: { name: "Test User", email: "test@example.com" },
  });

  await rm(path.join(dir, "content.txt"));
  await rm(path.join(dir, "empty.txt"));
  await stage(repo, [], true, false);
  const deleted = await diff(repo, [], [], true, false, 3);
  assert.match(
    deleted.patch,
    /diff --git a\/content\.txt b\/content\.txt\ndeleted file mode 100644\n--- a\/content\.txt\n\+\+\+ \/dev\/null/,
  );
  assert.match(
    deleted.patch,
    /diff --git a\/empty\.txt b\/empty\.txt\ndeleted file mode 100644\n--- a\/empty\.txt\n\+\+\+ \/dev\/null/,
  );
  assert.doesNotMatch(deleted.patch, /^=+$/m);
});

test("diff preserves UTF-8 BOM changes", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const filepath = path.join(dir, "bom.txt");
  await writeFile(filepath, "\uFEFFbefore\n");
  const repo = await repository([dir]);
  await stage(repo, ["bom.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "with BOM",
    author: { name: "Test User", email: "test@example.com" },
  });

  await writeFile(filepath, "before\n");
  const removed = await diff(repo, [], [], false, false, 3);
  assert.match(removed.patch, /-\uFEFFbefore\n\+before/);
  assert.match(removed.stat, /1 insertion\(\+\), 1 deletion\(-\)/);

  await writeFile(filepath, "\uFEFFafter\n");
  const edited = await diff(repo, [], [], false, false, 3);
  assert.match(edited.patch, /-\uFEFFbefore\n\+\uFEFFafter/);

  await writeFile(filepath, "before\n");
  await stage(repo, ["bom.txt"], false, false);
  await writeFile(filepath, "\uFEFFbefore\n");
  const added = await diff(repo, [], [], false, false, 3);
  assert.match(added.patch, /-before\n\+\uFEFFbefore/);
});

test("read-only status and diff cover deletions, binaries, path filters, stats, and trees", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, ".gitignore"), "ignored.txt\n");
  await writeFile(path.join(dir, "changed.txt"), "before\n");
  await writeFile(path.join(dir, "deleted.txt"), "deleted\n");
  await writeFile(path.join(dir, "binary.bin"), Buffer.from([0xff, 1, 2]));
  await writeFile(path.join(dir, "no-newline.txt"), "before");
  const repo = await repository([dir]);
  await stage(repo, [], true, false);
  const before = await git.commit({
    fs,
    dir,
    message: "before",
    author: { name: "Test User", email: "test@example.com" },
  });

  await writeFile(path.join(dir, "changed.txt"), "after\n");
  await rm(path.join(dir, "deleted.txt"));
  await writeFile(path.join(dir, "binary.bin"), Buffer.from([0xfe, 1, 3]));
  await writeFile(path.join(dir, "no-newline.txt"), "after");
  await writeFile(path.join(dir, "ignored.txt"), "ignored\n");

  assert.deepEqual(
    (await statusEntries(repo, [])).map((entry) => [entry.path, entry.code]),
    [
      ["binary.bin", " M"],
      ["changed.txt", " M"],
      ["deleted.txt", " D"],
      ["no-newline.txt", " M"],
    ],
  );
  assert.deepEqual(
    (await statusEntries(repo, ["changed.txt"])).map((entry) => entry.path),
    ["changed.txt"],
  );

  const worktree = await diff(repo, [], [], false, false, 1);
  assert.deepEqual(worktree.paths, [
    "binary.bin",
    "changed.txt",
    "deleted.txt",
    "no-newline.txt",
  ]);
  assert.match(
    worktree.patch,
    /Binary files a\/binary\.bin and b\/binary\.bin differ/,
  );
  assert.match(worktree.patch, /-before\n\+after/);
  assert.match(worktree.patch, /\\ No newline at end of file/);
  assert.match(worktree.stat, /4 files changed/);

  const filtered = await diff(repo, [], ["changed.txt"], false, true, 3);
  assert.equal(filtered.patch, "changed.txt\n");

  await stage(repo, [], true, false);
  const after = await git.commit({
    fs,
    dir,
    message: "after",
    author: { name: "Test User", email: "test@example.com" },
  });
  const trees = await diff(repo, [before, after], [], false, false, 3);
  assert.deepEqual(trees.paths, worktree.paths);
  assert.match(trees.patch, /-before\n\+after/);
});

test("diff handles file-directory replacements without reading symlink targets", async (t) => {
  const dir = await fixture();
  const symlinkDir = await fixture();
  const destination = await mkdtemp(
    path.join(os.tmpdir(), "golem-git-diff-destination-"),
  );
  t.after(() => rm(dir, { recursive: true, force: true }));
  t.after(() => rm(symlinkDir, { recursive: true, force: true }));
  t.after(() => rm(destination, { recursive: true, force: true }));
  await fs.promises.mkdir(path.join(dir, "a"));
  await writeFile(path.join(dir, "a/file.txt"), "nested\n");
  const repo = await repository([dir]);
  await stage(repo, [], true, false);
  const directoryCommit = await git.commit({
    fs,
    dir,
    message: "directory",
    author: { name: "Test User", email: "test@example.com" },
  });

  await rm(path.join(dir, "a"), { recursive: true });
  await writeFile(path.join(dir, "a"), "flat\n");
  await stage(repo, [], true, false);
  const fileCommit = await git.commit({
    fs,
    dir,
    message: "file",
    author: { name: "Test User", email: "test@example.com" },
  });
  const trees = await diff(
    repo,
    [directoryCommit, fileCommit],
    [],
    false,
    false,
    3,
  );
  assert.deepEqual(trees.paths, ["a", "a/file.txt"]);

  await fs.promises.mkdir(path.join(symlinkDir, "a"));
  await writeFile(path.join(symlinkDir, "a/file.txt"), "nested\n");
  const symlinkRepo = await repository([symlinkDir]);
  await stage(symlinkRepo, [], true, false);
  await git.commit({
    fs,
    dir: symlinkDir,
    message: "directory",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(path.join(destination, "file.txt"), "outside-secret\n");
  await rm(path.join(symlinkDir, "a"), { recursive: true });
  await symlink(destination, path.join(symlinkDir, "a"));
  const worktree = await diff(symlinkRepo, [], [], false, false, 3);
  assert.equal(worktree.patch.includes("outside-secret"), false);
  assert.ok(worktree.paths.includes("a/file.txt"));
});

test("diff stats count source lines beginning with header-like prefixes", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "prefix.txt"), "--old\n");
  const repo = await repository([dir]);
  await stage(repo, ["prefix.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "before",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(path.join(dir, "prefix.txt"), "++new\n");
  const result = await diff(repo, [], [], false, false, 3, true);
  assert.match(result.patch, /1 insertion\(\+\), 1 deletion\(-\)/);
});

test("diff rejects work above the deterministic edit budget", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const filepath = path.join(dir, "large.txt");
  await writeFile(filepath, `${"old\n".repeat(10_001)}`);
  const repo = await repository([dir]);
  await stage(repo, ["large.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "before",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(filepath, `${"new\n".repeat(10_001)}`);
  await assert.rejects(
    diff(repo, [], [], false, false, 3),
    /line computation limit/,
  );
});

test("relative symlinks retain link text and mode across staging and commits", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "target-a.txt"), "a\n");
  await writeFile(path.join(dir, "target-b.txt"), "b\n");
  await symlink("target-a.txt", path.join(dir, "link.txt"));
  const repo = await repository([dir]);

  await stage(repo, ["link.txt"], false, false);
  const oid = await git.commit({
    fs,
    dir,
    message: "add link",
    author: { name: "Test User", email: "test@example.com" },
  });
  const tree = await git.readTree({ fs, dir, oid });
  const entry = tree.tree.find((candidate) => candidate.path === "link.txt");
  assert.equal(entry?.mode, "120000");
  assert.equal(
    new TextDecoder().decode(
      (await git.readBlob({ fs, dir, oid, filepath: "link.txt" })).blob,
    ),
    "target-a.txt",
  );

  await rm(path.join(dir, "link.txt"));
  await symlink("target-b.txt", path.join(dir, "link.txt"));
  const changed = await diff(repo, [], ["link.txt"], false, false, 3);
  assert.equal(changed.hasChanges, true);
  assert.match(changed.patch, /-target-a\.txt/);
  assert.match(changed.patch, /\+target-b\.txt/);
  await stage(repo, ["link.txt"], false, false);
  assert.equal(
    (await lstat(path.join(dir, "link.txt"))).isSymbolicLink(),
    true,
  );
  assert.equal(await readlink(path.join(dir, "link.txt")), "target-b.txt");
});

test("same-content file and symlink transitions are status, diff, and staging changes", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "entry"), "target.txt");
  const repo = await repository([dir]);
  await stage(repo, ["entry"], false, false);
  await git.commit({
    fs,
    dir,
    message: "regular file",
    author: { name: "Test User", email: "test@example.com" },
  });

  await rm(path.join(dir, "entry"));
  await symlink("target.txt", path.join(dir, "entry"));
  assert.deepEqual(await statusEntries(repo, []), [
    { path: "entry", index: "unmodified", worktree: "modified", code: " M" },
  ]);
  assert.match(
    (await diff(repo, [], [], false, false, 3)).patch,
    /old mode 100644/,
  );
  assert.match(
    (await diff(repo, [], [], false, false, 3)).patch,
    /new mode 120000/,
  );

  await stage(repo, ["entry"], false, false);
  assert.deepEqual(await statusEntries(repo, []), [
    { path: "entry", index: "modified", worktree: "unmodified", code: "M " },
  ]);
});

test("path checkout restores same-content file and symlink transitions", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const entry = path.join(dir, "entry");
  await writeFile(entry, "target.txt");
  const repo = await repository([dir]);
  await stage(repo, ["entry"], false, false);

  await rm(entry);
  await symlink("target.txt", entry);
  assert.deepEqual(await restoreIndexPaths(repo, ["entry"]), ["entry"]);
  assert.equal((await lstat(entry)).isFile(), true);

  await rm(entry);
  await symlink("target.txt", entry);
  await stage(repo, ["entry"], false, false);
  await rm(entry);
  await writeFile(entry, "target.txt");
  await restoreIndexPaths(repo, ["entry"]);
  assert.equal((await lstat(entry)).isSymbolicLink(), true);
  assert.equal(await readlink(entry), "target.txt");
});

test("path checkout rejects symlinked parent directories before mutation", async (t) => {
  const dir = await fixture();
  const destination = await mkdtemp(
    path.join(os.tmpdir(), "golem-git-tool-destination-"),
  );
  t.after(() => rm(dir, { recursive: true, force: true }));
  t.after(() => rm(destination, { recursive: true, force: true }));
  await fs.promises.mkdir(path.join(dir, "src"));
  await writeFile(path.join(dir, "src/file.txt"), "indexed\n");
  const repo = await repository([dir]);
  await stage(repo, ["src/file.txt"], false, false);
  await rm(path.join(dir, "src"), { recursive: true });
  await writeFile(path.join(destination, "file.txt"), "unrelated\n");
  await symlink(destination, path.join(dir, "src"));

  await assert.rejects(
    restoreIndexPaths(repo, ["src"]),
    /symbolic-link directory/,
  );
  assert.equal((await lstat(path.join(dir, "src"))).isSymbolicLink(), true);
  assert.equal(
    await fs.promises.readFile(path.join(destination, "file.txt"), "utf8"),
    "unrelated\n",
  );
});

test("branch checkout rejects a conflicting local file type change", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const entry = path.join(dir, "entry");
  await writeFile(entry, "base");
  const repo = await repository([dir]);
  await stage(repo, ["entry"], false, false);
  const base = await git.commit({
    fs,
    dir,
    message: "base",
    author: { name: "Test User", email: "test@example.com" },
  });
  await git.branch({ fs, dir, ref: "target", object: base });
  await git.checkout({ fs, dir, ref: "target" });
  await writeFile(entry, "target");
  await stage(repo, ["entry"], false, false);
  const target = await git.commit({
    fs,
    dir,
    message: "target",
    author: { name: "Test User", email: "test@example.com" },
  });
  await git.checkout({ fs, dir, ref: "main", force: true });
  await rm(entry);
  await symlink("linked-target", entry);

  await assert.rejects(
    checkoutCommand(repo, undefined, false, "target", []),
    /local changes would be overwritten/,
  );
  assert.equal(await readlink(entry), "linked-target");
  assert.equal(await git.resolveRef({ fs, dir, ref: "HEAD" }), base);
});

test("checkout preservation rejects symlinked ancestors before mutation", async (t) => {
  const dir = await fixture();
  const outside = await mkdtemp(path.join(os.tmpdir(), "golem-git-outside-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  t.after(() => rm(outside, { recursive: true, force: true }));
  await writeFile(path.join(dir, "base.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["base.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);
  await fs.promises.mkdir(path.join(dir, "d"));
  await writeFile(path.join(dir, "d/victim.txt"), "indexed\n");
  await stage(repo, ["d/victim.txt"], false, false);
  await rm(path.join(dir, "d"), { recursive: true });
  await writeFile(path.join(outside, "victim.txt"), "outside\n");
  await symlink(outside, path.join(dir, "d"));

  await assert.rejects(
    checkoutCommand(repo, "new", false, undefined, []),
    /symbolic-link directory/,
  );
  assert.equal(
    await fs.promises.readFile(path.join(outside, "victim.txt"), "utf8"),
    "outside\n",
  );
  assert.equal((await git.listBranches({ fs, dir })).includes("new"), false);
});

test("checkout rejects ignored symlink ancestors for target paths", async (t) => {
  const dir = await fixture();
  const outside = await mkdtemp(path.join(os.tmpdir(), "golem-git-ignored-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  t.after(() => rm(outside, { recursive: true, force: true }));
  await writeFile(path.join(dir, "base.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["base.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);
  await branchCommand(repo, false, "target", undefined);
  await checkoutCommand(repo, undefined, false, "target", []);
  await fs.promises.mkdir(path.join(dir, "d"));
  await writeFile(path.join(dir, "d/victim.txt"), "target\n");
  await stage(repo, ["d/victim.txt"], false, false);
  await createCommit(repo, ["target"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);
  await writeFile(path.join(dir, ".gitignore"), "d\n");
  await writeFile(path.join(outside, "victim.txt"), "outside\n");
  await rm(path.join(dir, "d"), { recursive: true });
  await symlink(outside, path.join(dir, "d"));

  await assert.rejects(
    checkoutCommand(repo, undefined, false, "target", []),
    /symbolic-link directory/,
  );
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal((await lstat(path.join(dir, "d"))).isSymbolicLink(), true);
  assert.equal(
    await fs.promises.readFile(path.join(outside, "victim.txt"), "utf8"),
    "outside\n",
  );
});

test("checkout rejects file-directory tree transitions before mutation", async (t) => {
  for (const initialDirectory of [true, false]) {
    const dir = await fixture();
    t.after(() => rm(dir, { recursive: true, force: true }));
    const entry = path.join(dir, "a");
    if (initialDirectory) {
      await fs.promises.mkdir(entry);
      await writeFile(path.join(entry, "old.txt"), "directory\n");
    } else {
      await writeFile(entry, "file\n");
    }
    const repo = await repository([dir]);
    await stage(repo, [], true, false);
    const initial = await createCommit(repo, ["initial"], undefined, false);
    await branchCommand(repo, false, "initial", initial);

    await rm(entry, { recursive: true });
    if (initialDirectory) {
      await writeFile(entry, "file\n");
    } else {
      await fs.promises.mkdir(entry);
      await writeFile(path.join(entry, "old.txt"), "directory\n");
    }
    await stage(repo, [], true, false);
    const current = await createCommit(repo, ["current"], undefined, false);

    await assert.rejects(
      checkoutCommand(repo, undefined, false, "initial", []),
      /file and directory trees is not supported safely/,
    );
    assert.equal(await git.resolveRef({ fs, dir, ref: "HEAD" }), current);
    assert.deepEqual(await statusEntries(repo, []), []);
    assert.equal((await lstat(entry)).isDirectory(), !initialDirectory);
  }
});

test("branch mutations use one canonical local ref", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);

  await branchCommand(repo, false, "refs/heads/main", undefined);
  await branchCommand(repo, true, "refs/heads/main", undefined);
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal((await git.listBranches({ fs, dir })).includes("main"), true);
});

test("checkout -b cannot be redirected by a same-named tag", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "base.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["base.txt"], false, false);
  const base = await createCommit(repo, ["base"], undefined, false);
  await branchCommand(repo, false, "tag-target", undefined);
  await checkoutCommand(repo, undefined, false, "tag-target", []);
  await writeFile(path.join(dir, "victim.txt"), "tag\n");
  await stage(repo, ["victim.txt"], false, false);
  const tagTarget = await createCommit(repo, ["tag target"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);
  await git.writeRef({ fs, dir, ref: "refs/tags/new", value: tagTarget });
  await writeFile(path.join(dir, "victim.txt"), "untracked\n");

  await checkoutCommand(repo, "new", false, undefined, []);
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "new");
  assert.equal(await git.resolveRef({ fs, dir, ref: "HEAD" }), base);
  assert.equal(
    await fs.promises.readFile(path.join(dir, "victim.txt"), "utf8"),
    "untracked\n",
  );
});

test("ordinary checkout resolves a same-named local branch before its tag", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "base.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["base.txt"], false, false);
  const base = await createCommit(repo, ["base"], undefined, false);
  await branchCommand(repo, false, "target", undefined);
  await checkoutCommand(repo, undefined, false, "target", []);
  await writeFile(path.join(dir, "victim.txt"), "branch\n");
  await stage(repo, ["victim.txt"], false, false);
  await createCommit(repo, ["branch target"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);
  await git.writeRef({ fs, dir, ref: "refs/tags/target", value: base });
  await writeFile(path.join(dir, "victim.txt"), "untracked\n");

  await assert.rejects(
    checkoutCommand(repo, undefined, false, "target", []),
    /would be overwritten/,
  );
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal(
    await fs.promises.readFile(path.join(dir, "victim.txt"), "utf8"),
    "untracked\n",
  );
});

test("checkout HEAD keeps the current branch attached", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);

  await checkoutCommand(repo, undefined, false, "HEAD", []);
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");

  await writeFile(path.join(dir, "file.txt"), "next\n");
  await stage(repo, ["file.txt"], false, false);
  const next = await createCommit(repo, ["next"], undefined, false);
  assert.equal(await git.resolveRef({ fs, dir, ref: "refs/heads/main" }), next);
});

test("checkout never writes through a preserved leaf symlink", async (t) => {
  const dir = await fixture();
  const outside = await mkdtemp(path.join(os.tmpdir(), "golem-git-victim-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  t.after(() => rm(outside, { recursive: true, force: true }));
  const victim = path.join(outside, "victim.txt");
  const entry = path.join(dir, "entry");
  await writeFile(victim, "victim\n");
  await writeFile(entry, "head\n");
  const repo = await repository([dir]);
  await stage(repo, ["entry"], false, false);
  await createCommit(repo, ["head"], undefined, false);
  await writeFile(entry, victim);
  await stage(repo, ["entry"], false, false);
  await rm(entry);
  await symlink(victim, entry);

  await checkoutCommand(repo, "new", false, undefined, []);
  assert.equal(await fs.promises.readFile(victim, "utf8"), "victim\n");
  assert.equal((await lstat(entry)).isSymbolicLink(), true);
  assert.equal(await readlink(entry), victim);
});

test("late checkout failure restores the complete original worktree", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await symlink("old", path.join(dir, "link"));
  const repo = await repository([dir]);
  await stage(repo, ["link"], false, false);
  await createCommit(repo, ["old link"], undefined, false);
  await branchCommand(repo, false, "target", undefined);
  await checkoutCommand(repo, undefined, false, "target", []);
  await rm(path.join(dir, "link"));
  await symlink("new", path.join(dir, "link"));
  await writeFile(path.join(dir, "new.txt"), "new\n");
  await stage(repo, [], true, false);
  await createCommit(repo, ["target"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);

  const originalSymlink = fs.promises.symlink;
  fs.promises.symlink = async (target, filepath, type) => {
    if (target === "new" && filepath === path.join(dir, "link")) {
      const error = new Error("injected symlink failure") as NodeJS.ErrnoException;
      error.code = "EIO";
      throw error;
    }
    return originalSymlink(target, filepath, type);
  };
  try {
    await assert.rejects(checkoutCommand(repo, "failed", false, "target", []));
  } finally {
    fs.promises.symlink = originalSymlink;
  }

  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal((await git.listBranches({ fs, dir })).includes("failed"), false);
  assert.equal(await readlink(path.join(dir, "link")), "old");
  await assert.rejects(fs.promises.lstat(path.join(dir, "new.txt")), {
    code: "ENOENT",
  });
});

test("branch start points must name existing commits", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);
  const blob = await git.writeBlob({ fs, dir, blob: Buffer.from("blob") });

  await assert.rejects(
    branchCommand(repo, false, "missing", "0".repeat(40)),
    /not an existing commit/,
  );
  await assert.rejects(
    branchCommand(repo, false, "blob", blob),
    /not an existing commit/,
  );
  assert.deepEqual(await git.listBranches({ fs, dir }), ["main"]);
});

test("checkout validates target blobs before preserving local state", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "base.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["base.txt"], false, false);
  await createCommit(repo, ["base"], undefined, false);
  await branchCommand(repo, false, "target", undefined);
  await checkoutCommand(repo, undefined, false, "target", []);
  await writeFile(path.join(dir, "z.txt"), "target\n");
  await stage(repo, ["z.txt"], false, false);
  const target = await createCommit(repo, ["target"], undefined, false);
  const { oid: targetBlob } = await git.readBlob({
    fs,
    dir,
    oid: target,
    filepath: "z.txt",
  });
  await checkoutCommand(repo, undefined, false, "main", []);
  await rm(
    path.join(
      dir,
      ".git/objects",
      targetBlob.slice(0, 2),
      targetBlob.slice(2),
    ),
  );
  await writeFile(path.join(dir, "local.txt"), "index\n");
  await stage(repo, ["local.txt"], false, false);
  await writeFile(path.join(dir, "local.txt"), "worktree\n");

  await assert.rejects(
    checkoutCommand(repo, "failed", false, "target", []),
  );
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal((await git.listBranches({ fs, dir })).includes("failed"), false);
  assert.deepEqual(await statusEntries(repo, ["local.txt"]), [
    {
      path: "local.txt",
      index: "added",
      worktree: "modified",
      code: "AM",
    },
  ]);
  assert.equal(
    await fs.promises.readFile(path.join(dir, "local.txt"), "utf8"),
    "worktree\n",
  );
});

test("branch creation, listing, and guarded deletion follow HEAD ancestry", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  const base = await createCommit(repo, ["base"], undefined, false);

  assert.deepEqual(await branchCommand(repo, false, "same-tip", undefined), [
    { name: "main", current: true, oid: base },
    { name: "same-tip", current: false, oid: base },
  ]);
  assert.deepEqual(await branchCommand(repo, true, "same-tip", undefined), [
    { name: "main", current: true, oid: base },
  ]);
  await assert.rejects(
    branchCommand(repo, true, "main", undefined),
    /cannot delete branch/,
  );

  await branchCommand(repo, false, "feature", undefined);
  await checkoutCommand(repo, undefined, false, "feature", []);
  await writeFile(path.join(dir, "file.txt"), "feature\n");
  await stage(repo, ["file.txt"], false, false);
  const feature = await createCommit(repo, ["feature"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);
  await assert.rejects(
    branchCommand(repo, true, "feature", undefined),
    /not fully merged/,
  );
  assert.equal(
    await git.resolveRef({ fs, dir, ref: "refs/heads/feature" }),
    feature,
  );

  await branchCommand(repo, false, "ancestor", undefined);
  await writeFile(path.join(dir, "main.txt"), "main\n");
  await stage(repo, ["main.txt"], false, false);
  await createCommit(repo, ["main"], undefined, false);
  const remaining = await branchCommand(repo, true, "ancestor", undefined);
  assert.deepEqual(
    remaining.map((entry) => entry.name),
    ["feature", "main"],
  );
});

test("checkout preserves permissible changes, rejects overwrites, cleans failed -b, and detaches", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "shared.txt"), "shared\n");
  await writeFile(path.join(dir, "conflict.txt"), "base\n");
  const repo = await repository([dir]);
  await stage(repo, [], true, false);
  const base = await createCommit(repo, ["base"], undefined, false);
  await branchCommand(repo, false, "target", undefined);
  await checkoutCommand(repo, undefined, false, "target", []);
  await writeFile(path.join(dir, "conflict.txt"), "target\n");
  await stage(repo, ["conflict.txt"], false, false);
  await createCommit(repo, ["target"], undefined, false);
  await checkoutCommand(repo, undefined, false, "main", []);

  await writeFile(path.join(dir, "conflict.txt"), "local\n");
  await assert.rejects(
    checkoutCommand(repo, undefined, false, "target", []),
    /would be overwritten/,
  );
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), "main");
  assert.equal(
    await fs.promises.readFile(path.join(dir, "conflict.txt"), "utf8"),
    "local\n",
  );
  await assert.rejects(
    checkoutCommand(repo, "failed", false, "target", []),
    /would be overwritten/,
  );
  assert.equal((await git.listBranches({ fs, dir })).includes("failed"), false);

  await writeFile(path.join(dir, "conflict.txt"), "base\n");
  await writeFile(path.join(dir, "shared.txt"), "staged-local\n");
  await stage(repo, ["shared.txt"], false, false);
  await checkoutCommand(repo, undefined, false, "target", []);
  assert.equal(
    await fs.promises.readFile(path.join(dir, "shared.txt"), "utf8"),
    "staged-local\n",
  );
  assert.equal((await statusEntries(repo, []))[0]?.code, "M ");

  const detached = await checkoutCommand(repo, undefined, true, base, []);
  assert.match(detached.summary, /HEAD is now at/);
  assert.equal(await git.currentBranch({ fs, dir, fullname: false }), undefined);
  assert.equal(await git.resolveRef({ fs, dir, ref: "HEAD" }), base);
});

test("path checkout validates every path and materializes every blob before mutation", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "a.txt"), "indexed-a\n");
  await writeFile(path.join(dir, "z.txt"), "indexed-z\n");
  const repo = await repository([dir]);
  await stage(repo, [], true, false);
  const commit = await createCommit(repo, ["files"], undefined, false);
  await writeFile(path.join(dir, "a.txt"), "worktree-a\n");
  await assert.rejects(
    restoreIndexPaths(repo, ["a.txt", "missing.txt"]),
    /pathspec did not match/,
  );
  assert.equal(
    await fs.promises.readFile(path.join(dir, "a.txt"), "utf8"),
    "worktree-a\n",
  );

  const { oid: blobOid } = await git.readBlob({
    fs,
    dir,
    oid: commit,
    filepath: "z.txt",
  });
  await rm(
    path.join(dir, ".git/objects", blobOid.slice(0, 2), blobOid.slice(2)),
  );
  await assert.rejects(restoreIndexPaths(repo, ["a.txt", "z.txt"]));
  assert.equal(
    await fs.promises.readFile(path.join(dir, "a.txt"), "utf8"),
    "worktree-a\n",
  );
});

test("diff root pathspec and cached diff on unborn HEAD include changes", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, "file.txt"), "new\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);

  const unfiltered = await diff(repo, [], [], true, false, 3);
  const fromRoot = await diff(repo, [], ["."], true, false, 3);
  assert.equal(fromRoot.patch, unfiltered.patch);
  assert.deepEqual(fromRoot.paths, ["file.txt"]);
});

test("all diff output forms enforce the byte limit", () => {
  assert.equal(limitedDiffOutput("small"), "small");
  assert.throws(
    () => limitedDiffOutput("x".repeat(MAX_DIFF_BYTES + 1)),
    /diff exceeds/,
  );
});

test("ignored executables do not block add --all", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, ".gitignore"), "ignored\n");
  await writeFile(path.join(dir, "tracked.txt"), "tracked\n");
  const ignored = path.join(dir, "ignored");
  await writeFile(ignored, "#!/bin/sh\n");
  await chmod(ignored, 0o755);
  const repo = await repository([dir]);

  assert.deepEqual(await stage(repo, [], true, false), [
    ".gitignore",
    "tracked.txt",
  ]);
  assert.deepEqual(await git.listFiles({ fs, dir }), [
    ".gitignore",
    "tracked.txt",
  ]);
});

test("add handles explicit paths, ignored descendants, -A, and -u", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await fs.promises.mkdir(path.join(dir, "src"));
  await writeFile(path.join(dir, ".gitignore"), "src/ignored.txt\nignored-dir/\n");
  await writeFile(path.join(dir, "src/tracked.txt"), "before\n");
  await writeFile(path.join(dir, "src/ignored.txt"), "ignored\n");
  await fs.promises.mkdir(path.join(dir, "ignored-dir"));
  await writeFile(path.join(dir, "ignored-dir/file.txt"), "ignored\n");
  const repo = await repository([dir]);

  assert.deepEqual(await stage(repo, ["src"], false, false), [
    "src/tracked.txt",
  ]);
  await stage(repo, [".gitignore"], false, false);
  await git.commit({
    fs,
    dir,
    message: "initial",
    author: { name: "Test User", email: "test@example.com" },
  });

  await writeFile(path.join(dir, "src/tracked.txt"), "after\n");
  await writeFile(path.join(dir, "new.txt"), "new\n");
  assert.deepEqual(await stage(repo, [], false, true), ["src/tracked.txt"]);
  assert.deepEqual(await git.listFiles({ fs, dir }), [
    ".gitignore",
    "src/tracked.txt",
  ]);

  await rm(path.join(dir, "src/tracked.txt"));
  assert.deepEqual(await stage(repo, [], true, false), [
    "new.txt",
    "src/tracked.txt",
  ]);
  assert.deepEqual(await git.listFiles({ fs, dir }), [".gitignore", "new.txt"]);
});

test("explicit missing and ignored paths fail before changing the index", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  await writeFile(path.join(dir, ".gitignore"), "ignored.txt\n");
  await writeFile(path.join(dir, "valid.txt"), "valid\n");
  await writeFile(path.join(dir, "ignored.txt"), "ignored\n");
  const repo = await repository([dir]);

  await assert.rejects(
    stage(repo, ["valid.txt", "missing.txt"], false, false),
    /pathspec did not match/,
  );
  assert.deepEqual(await git.listFiles({ fs, dir }), []);
  await assert.rejects(
    stage(repo, ["valid.txt", "ignored.txt"], false, false),
    /following path is ignored/,
  );
  assert.deepEqual(await git.listFiles({ fs, dir }), []);
});

test("repeated -A preserves a staged file-to-directory replacement", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const entry = path.join(dir, "entry");
  await writeFile(entry, "file\n");
  const repo = await repository([dir]);
  await stage(repo, ["entry"], false, false);
  await git.commit({
    fs,
    dir,
    message: "file",
    author: { name: "Test User", email: "test@example.com" },
  });

  await rm(entry);
  await fs.promises.mkdir(entry);
  await writeFile(path.join(entry, "child.txt"), "child\n");
  assert.deepEqual(await stage(repo, [], true, false), [
    "entry",
    "entry/child.txt",
  ]);
  assert.deepEqual(await git.listFiles({ fs, dir }), ["entry/child.txt"]);
  assert.deepEqual(await stage(repo, [], true, false), []);
  assert.deepEqual(await git.listFiles({ fs, dir }), ["entry/child.txt"]);
});

test("-u does not re-add a file recreated after its staged deletion", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const filepath = path.join(dir, "file.txt");
  await writeFile(filepath, "tracked\n");
  const repo = await repository([dir]);
  await stage(repo, ["file.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "tracked",
    author: { name: "Test User", email: "test@example.com" },
  });
  await rm(filepath);
  await stage(repo, [], true, false);
  await writeFile(filepath, "recreated\n");

  assert.deepEqual(await stage(repo, [], false, true), []);
  assert.deepEqual(await git.listFiles({ fs, dir }), []);
});

test("HEAD-only ignored paths reject explicit add atomically", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const ignored = path.join(dir, "ignored.txt");
  await writeFile(ignored, "tracked\n");
  const repo = await repository([dir]);
  await stage(repo, ["ignored.txt"], false, false);
  await git.commit({
    fs,
    dir,
    message: "tracked",
    author: { name: "Test User", email: "test@example.com" },
  });
  await rm(ignored);
  await stage(repo, [], true, false);
  await writeFile(path.join(dir, ".gitignore"), "ignored.txt\n");
  await writeFile(ignored, "recreated\n");
  await writeFile(path.join(dir, "valid.txt"), "valid\n");

  await assert.rejects(
    stage(repo, ["valid.txt", "ignored.txt"], false, false),
    /following path is ignored/,
  );
  assert.deepEqual(await git.listFiles({ fs, dir }), []);
});

test("chmod-only changes and executable checkout targets fail closed", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const executable = path.join(dir, "run.sh");
  await writeFile(executable, "#!/bin/sh\n");
  const repo = await repository([dir]);
  await stage(repo, ["run.sh"], false, false);
  await git.commit({
    fs,
    dir,
    message: "non-executable",
    author: { name: "Test User", email: "test@example.com" },
  });

  await chmod(executable, 0o755);
  await assert.rejects(
    statusEntries(repo, []),
    /cannot preserve executable-file modes/,
  );
  await assert.rejects(
    diff(repo, [], [], false, true, 3),
    /cannot preserve executable-file modes/,
  );
  await assert.rejects(
    stage(repo, ["run.sh"], false, false),
    /cannot preserve executable-file modes/,
  );
  await git.add({ fs, dir, filepath: "run.sh" });
  await assert.rejects(
    statusEntries(repo, []),
    /cannot preserve executable-file modes/,
  );
  const executableCommit = await git.commit({
    fs,
    dir,
    message: "executable",
    author: { name: "Test User", email: "test@example.com" },
  });
  await assert.rejects(
    assertRefSupportedModes(repo, executableCommit),
    /cannot preserve executable-file modes/,
  );
  await chmod(executable, 0o644);
  await git.add({ fs, dir, filepath: "run.sh" });
  await assert.rejects(
    diff(repo, [executableCommit], [], false, true, 3),
    /cannot preserve executable-file modes/,
  );
});

test("executable files fail closed before staging", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const executable = path.join(dir, "run.sh");
  await writeFile(executable, "#!/bin/sh\n");
  await chmod(executable, 0o755);
  const repo = await repository([dir]);
  await assert.rejects(
    stage(repo, ["run.sh"], false, false),
    /cannot preserve executable-file modes/,
  );
  assert.deepEqual(await git.listFiles({ fs, dir }), []);
});

test("diff quotes paths that contain record-separator characters", async (t) => {
  const dir = await fixture();
  t.after(() => rm(dir, { recursive: true, force: true }));
  const filepath = "line\nbreak.txt";
  await writeFile(path.join(dir, filepath), "before\n");
  const repo = await repository([dir]);
  await stage(repo, [filepath], false, false);
  await git.commit({
    fs,
    dir,
    message: "initial",
    author: { name: "Test User", email: "test@example.com" },
  });
  await writeFile(path.join(dir, filepath), "after\n");

  const result = await diff(repo, [], [], false, false, 3);
  assert.match(
    result.patch,
    /^diff --git "a\/line\\nbreak\.txt" "b\/line\\nbreak\.txt"$/m,
  );
  assert.match(result.patch, /^--- "a\/line\\nbreak\.txt"$/m);
  assert.match(result.patch, /^\+\+\+ "b\/line\\nbreak\.txt"$/m);
  assert.equal(result.paths[0], filepath);
});
