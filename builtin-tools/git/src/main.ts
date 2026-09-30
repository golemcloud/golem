import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import { c, err, ok, toolDefinition } from "@golemcloud/golem-ts-sdk";
import { z } from "zod/v4";
import {
  branchCommand,
  checkoutCommand,
  createCommit,
  diff,
  formatLog,
  formatStatus,
  localConfig,
  repository,
  resolveDocumentedRef,
  stage,
  statusEntries,
  validatedCwd,
} from "./git.js";

const failure = z.string();
const mutation = z.object({ summary: z.string(), paths: z.array(z.string()) });
const statusResult = z.object({
  entries: z.array(
    z.object({
      path: z.string(),
      index: z.string(),
      worktree: z.string(),
      code: z.string(),
    }),
  ),
  stdout: z.string(),
});
const diffResult = z.object({
  patch: z.string(),
  stat: z.string(),
  paths: z.array(z.string()),
  hasChanges: z.boolean(),
});
const commitResult = z.object({ oid: z.string(), summary: z.string() });
const logEntry = z.object({
  oid: z.string(),
  message: z.string(),
  authorName: z.string(),
  authorEmail: z.string(),
  timestamp: z.number(),
});
const logResult = z.object({ commits: z.array(logEntry), stdout: z.string() });
const branchResult = z.array(
  z.object({
    name: z.string(),
    current: z.boolean(),
    oid: z.string().optional(),
  }),
);

const definition = toolDefinition("git", { requiresFilesystem: true })
  .version("0.1.0")
  .doc({
    summary: "Run local Git workflows.",
    description:
      "A local-only Git command subset backed by isomorphic-git. Supports repository inspection, staging, commits, branches, checkout, and local identity configuration. It never fetches, pulls, pushes, or opens a remote. The shared tool interface requires long names for short-only Git options and accepts inherited -C after a subcommand.",
    examples: [
      {
        title: "Inspect a repository",
        body: "git -C workspace status --short",
      },
      {
        title: "Stage and commit a path",
        body: "git -C workspace add -- src/main.ts\ngit -C workspace commit -m \"Update main\"",
      },
    ],
  })
  .global("working-directory", z.string(), {
    short: "C",
    repeatable: "repeated",
    valueName: "PATH",
    doc: {
      summary: "Run as if Git started in PATH.",
      description:
        "May be repeated. Each relative PATH is resolved from the result of the previous -C, matching Git.",
    },
  })
  .command("init", (command) =>
    command
      .doc({
        summary: "Create an empty repository.",
        description:
          "Creates a normal working-tree repository in the selected directory.",
        examples: [{ title: "Initialize", body: "git init -b main workspace" }],
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("initial-branch", z.string(), {
            short: "b",
            valueName: "BRANCH",
            doc: "Name the initial branch.",
          })
          .positional("directory", z.string(), {
            required: false,
            valueName: "DIRECTORY",
            doc: "Directory to initialize; defaults to the effective working directory.",
          })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("status", (command) =>
    command
      .doc({
        summary: "Show index and worktree changes.",
        description:
          "Returns structured entries plus Git porcelain-v1 output. Paths are literal paths, not pathspec expressions.",
      })
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("short", { short: "s", doc: "Request compact status output." })
          .option("porcelain", z.literal("v1"), {
            optionalScalar: true,
            default: "v1",
            valueName: "VERSION",
            doc: "Use stable porcelain output. Only v1 is supported.",
          })
          .flag("null", { short: "z", doc: "Terminate output records with NUL." })
          .tail("paths", z.string(), {
            valueName: "PATH",
            doc: "Limit status to literal repository paths.",
          })
          .returns(statusResult)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("diff", (command) =>
    command
      .doc({
        summary: "Compare repository snapshots.",
        description:
          "With no references, compares the index to the worktree. --cached compares HEAD to the index. One reference compares that tree to the worktree; two compare tree to tree. Output limits fail explicitly rather than truncate patches.",
        examples: [
          { title: "Staged changes", body: "git diff --cached" },
          { title: "One path", body: "git diff -- src/main.ts" },
        ],
      })
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("cached", {
            aliases: ["staged"],
            doc: "Compare HEAD with the index.",
          })
          .flag("name-only", { doc: "Return only changed path names." })
          .flag("stat", { doc: "Return a diffstat instead of a patch." })
          .option("unified", z.number().int().min(0).max(1000), {
            short: "U",
            default: 3,
            valueName: "LINES",
            doc: "Show LINES of unified context.",
          })
          .positional("from", z.string(), {
            required: false,
            valueName: "FROM",
            doc: "Starting branch, tag, HEAD, or full object ID.",
          })
          .positional("to", z.string(), {
            required: false,
            valueName: "TO",
            doc: "Ending branch, tag, HEAD, or full object ID.",
          })
          .tail("paths", z.string(), {
            separator: "--",
            verbatim: true,
            valueName: "PATH",
            doc: "Literal paths to compare; pathspec magic is unsupported.",
          })
          .constraint(
            c.implies({ lhs: c.present("to"), rhs: c.present("from") }),
          )
          .constraint(
            c.forbids({
              lhs: c.present("name-only"),
              rhs: c.present("stat"),
            }),
          )
          .returns(diffResult)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("log", (command) =>
    command
      .doc({
        summary: "Show commit history.",
        description:
          "Reads history from HEAD or a documented branch, tag, or full object ID.",
      })
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("max-count", z.number().int().positive(), {
            short: "n",
            default: 20,
            valueName: "COUNT",
            doc: "Limit the number of commits returned.",
          })
          .flag("oneline", { doc: "Format each commit on one line." })
          .positional("ref", z.string(), {
            required: false,
            valueName: "REF",
            doc: "History starting point; defaults to HEAD.",
          })
          .returns(logResult)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("branch", (command) =>
    command
      .doc({
        summary: "List, create, or safely delete local branches.",
        description:
          "Deletion rejects the current branch and branches not merged into HEAD. Force deletion is not supported.",
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("delete", { short: "d", doc: "Delete a fully merged branch." })
          .positional("name", z.string(), {
            required: false,
            valueName: "BRANCH",
            doc: "Branch to create or delete; omit to list branches.",
          })
          .positional("start-point", z.string(), {
            required: false,
            valueName: "START_POINT",
            doc: "Commit used to create the branch; defaults to HEAD.",
          })
          .constraint(
            c.implies({
              lhs: c.present("start-point"),
              rhs: c.present("name"),
            }),
          )
          .returns(branchResult)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("add", (command) =>
    command
      .doc({
        summary: "Stage worktree changes.",
        description:
          "Stages explicit literal paths and directories, all changes with -A, or tracked changes with -u. Explicit ignored untracked paths are rejected.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("all", { short: "A", doc: "Stage all additions, changes, and deletions." })
          .flag("update", { short: "u", doc: "Stage changes and deletions to tracked paths." })
          .tail("paths", z.string(), {
            valueName: "PATH",
            doc: "Literal files or directories to stage.",
          })
          .constraint(
            c.forbids({ lhs: c.present("all"), rhs: c.present("update") }),
          )
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("commit", (command) =>
    command
      .doc({
        summary: "Record the staged tree.",
        description:
          "Uses user.name and user.email from local configuration for the committer. Repeated -m values form separate paragraphs.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("message", z.string(), {
            short: "m",
            repeatable: "repeated",
            required: true,
            valueName: "MESSAGE",
            doc: "Commit message paragraph; may be repeated.",
          })
          .option("author", z.string(), {
            valueName: "NAME <EMAIL>",
            doc: "Override the author while retaining the configured committer.",
          })
          .flag("allow-empty", { doc: "Create a commit even when the tree is unchanged." })
          .returns(commitResult)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("checkout", (command) =>
    command
      .doc({
        summary: "Switch branches or restore paths.",
        description:
          "Branch switching preserves permissible local changes and rejects overwrites. Paths after -- are destructively restored from the index. Force checkout and file/directory shape transitions are unsupported.",
        examples: [
          { title: "Create a branch", body: "git checkout -b fix-validation" },
          { title: "Restore a path", body: "git checkout -- src/main.ts" },
        ],
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("new-branch", z.string(), {
            short: "b",
            valueName: "BRANCH",
            doc: "Create and switch to a new branch.",
          })
          .flag("detach", { doc: "Check out a commit with detached HEAD." })
          .positional("ref", z.string(), {
            required: false,
            valueName: "REF",
            doc: "Branch, tag, HEAD, or full object ID to check out.",
          })
          .tail("paths", z.string(), {
            separator: "--",
            verbatim: true,
            valueName: "PATH",
            doc: "Literal paths to restore destructively from the index.",
          })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("config", (command) =>
    command
      .doc({
        summary: "Read or set local commit identity.",
        description:
          "Supports only user.name and user.email in the repository-local configuration.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("local", { default: true, doc: "Use repository-local configuration." })
          .positional("key", z.string(), {
            valueName: "KEY",
            doc: "user.name or user.email.",
          })
          .positional("value", z.string(), {
            required: false,
            valueName: "VALUE",
            doc: "New value; omit to read the current value.",
          })
          .returns(
            z.object({
              key: z.string(),
              value: z.string().optional(),
              updated: z.boolean(),
            }),
          )
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .implement({
    init: async ({ workingDirectory, directory, initialBranch }) =>
      guard(async () => {
        const dir = path.resolve(
          await validatedCwd(workingDirectory ?? []),
          directory ?? ".",
        );
        await git.init({ fs, dir, defaultBranch: initialBranch ?? "main" });
        return {
          summary: `Initialized empty Git repository in ${path.join(dir, ".git")}`,
          paths: [],
        };
      }),
    status: async ({ workingDirectory, paths, null: zero }) =>
      guard(async () => {
        const entries = await statusEntries(
          await repository(workingDirectory ?? []),
          paths,
        );
        return { entries, stdout: formatStatus(entries, zero) };
      }),
    diff: async ({
      workingDirectory,
      from,
      to,
      cached,
      nameOnly,
      stat,
      unified,
      paths,
    }) =>
      guard(async () => {
        const refs = [from, to].filter(
          (value): value is string => value !== undefined,
        );
        const result = await diff(
          await repository(workingDirectory ?? []),
          refs,
          paths,
          cached,
          nameOnly,
          unified,
          stat,
        );
        return result;
      }),
    log: async ({ workingDirectory, ref, maxCount, oneline }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        const resolved = ref ? await resolveDocumentedRef(repo, ref) : "HEAD";
        const commits = await git.log({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          ref: resolved,
          depth: maxCount,
        });
        const entries = commits.map(({ oid, commit }) => ({
          oid,
          message: commit.message,
          authorName: commit.author.name,
          authorEmail: commit.author.email,
          timestamp: commit.author.timestamp,
        }));
        return { commits: entries, stdout: formatLog(entries, oneline) };
      }),
    branch: async ({
      workingDirectory,
      delete: shouldDelete,
      name,
      startPoint,
    }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        return branchCommand(repo, shouldDelete, name, startPoint);
      }),
    add: async ({ workingDirectory, paths, all, update }) =>
      guard(async () => {
        if (all && update) throw new Error("-A and -u are mutually exclusive");
        const changed = await stage(
          await repository(workingDirectory ?? []),
          paths,
          all,
          update,
        );
        return {
          summary: changed.length
            ? `staged ${changed.length} path(s)`
            : "no paths changed",
          paths: changed,
        };
      }),
    commit: async ({ workingDirectory, message, author, allowEmpty }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        const oid = await createCommit(repo, message, author, allowEmpty);
        return { oid, summary: message[0] ?? "" };
      }),
    checkout: async ({ workingDirectory, newBranch, detach, ref, paths }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        return checkoutCommand(repo, newBranch, detach, ref, paths);
      }),
    config: async ({ workingDirectory, local, key, value }) =>
      guard(async () => {
        if (!local) throw new Error("only --local configuration is supported");
        const repo = await repository(workingDirectory ?? []);
        const configured = await localConfig(repo, key, value);
        return { key, value: configured, updated: value !== undefined };
      }),
  });

async function guard<T>(operation: () => Promise<T>) {
  try {
    return ok(await operation());
  } catch (error) {
    return err(
      "git-error",
      error instanceof Error ? error.message : String(error),
    );
  }
}

export { definition };
