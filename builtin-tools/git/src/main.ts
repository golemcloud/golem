import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import { err, ok, toolDefinition } from "@golemcloud/golem-ts-sdk";
import { z } from "zod/v4";
import {
  assertSupportedModes,
  diff,
  effectiveCwd,
  formatStatus,
  localIdentity,
  parseIdentity,
  relativePaths,
  repository,
  resolveDocumentedRef,
  stage,
  statusEntries,
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
  paths: z.array(z.string()),
  hasChanges: z.boolean(),
});
const commitResult = z.object({ oid: z.string(), summary: z.string() });
const logResult = z.array(
  z.object({
    oid: z.string(),
    message: z.string(),
    authorName: z.string(),
    authorEmail: z.string(),
    timestamp: z.number(),
  }),
);
const branchResult = z.array(
  z.object({
    name: z.string(),
    current: z.boolean(),
    oid: z.string().optional(),
  }),
);

const definition = toolDefinition("git", { requiresFilesystem: true })
  .version("0.1.0")
  .doc(
    "Local Git workflows backed by isomorphic-git. Remote operations are not exposed.",
  )
  .global("working-directory", z.string(), {
    short: "C",
    repeatable: "repeated",
    doc: "Run as if started in this directory. Repeated values apply in order.",
  })
  .command("init", (command) =>
    command
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("initial-branch", z.string(), { short: "b" })
          .positional("directory", z.string(), { required: false })
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
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("short", { short: "s" })
          .option("porcelain", z.literal("v1"), {
            optionalScalar: true,
            default: "v1",
          })
          .flag("null", { short: "z" })
          .tail("paths", z.string())
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
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("cached", { aliases: ["staged"] })
          .flag("name-only")
          .option("unified", z.number().int().min(0).max(1000), {
            short: "U",
            default: 3,
          })
          .positional("from", z.string(), { required: false })
          .positional("to", z.string(), { required: false })
          .tail("paths", z.string(), { separator: "--", verbatim: true })
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
          })
          .flag("oneline")
          .positional("ref", z.string(), { required: false })
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
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("delete", { short: "d" })
          .positional("name", z.string(), { required: false })
          .positional("start-point", z.string(), { required: false })
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
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("all", { short: "A" })
          .flag("update", { short: "u" })
          .tail("paths", z.string())
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
          })
          .option("author", z.string())
          .flag("allow-empty")
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
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .option("new-branch", z.string(), { short: "b" })
          .flag("detach")
          .positional("ref", z.string(), { required: false })
          .tail("paths", z.string(), { separator: "--", verbatim: true })
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
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("local", { default: true })
          .positional("key", z.string())
          .positional("value", z.string(), { required: false })
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
          effectiveCwd(workingDirectory ?? []),
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
      unified,
      paths,
    }) =>
      guard(async () => {
        const refs = [from, to].filter(
          (value): value is string => value !== undefined,
        );
        return diff(
          await repository(workingDirectory ?? []),
          refs,
          paths,
          cached,
          nameOnly,
          unified,
        );
      }),
    log: async ({ workingDirectory, ref, maxCount }) =>
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
        return commits.map(({ oid, commit }) => ({
          oid,
          message: commit.message,
          authorName: commit.author.name,
          authorEmail: commit.author.email,
          timestamp: commit.author.timestamp,
        }));
      }),
    branch: async ({
      workingDirectory,
      delete: shouldDelete,
      name,
      startPoint,
    }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        await assertSupportedModes(repo);
        const current = await git.currentBranch({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          fullname: false,
        });
        if (shouldDelete) {
          if (!name || startPoint)
            throw new Error("branch -d requires exactly one branch name");
          if (name === current)
            throw new Error(
              `cannot delete branch '${name}' checked out at '${repo.dir}'`,
            );
          const branchOid = await resolveDocumentedRef(repo, name);
          const headOid = await resolveDocumentedRef(repo, "HEAD");
          if (
            !(await git.isDescendent({
              fs,
              dir: repo.dir,
              gitdir: repo.gitdir,
              oid: headOid,
              ancestor: branchOid,
            }))
          )
            throw new Error(`branch '${name}' is not fully merged`);
          await git.deleteBranch({
            fs,
            dir: repo.dir,
            gitdir: repo.gitdir,
            ref: name,
          });
        } else if (name) {
          const object = startPoint
            ? await resolveDocumentedRef(repo, startPoint)
            : undefined;
          await git.branch({
            fs,
            dir: repo.dir,
            gitdir: repo.gitdir,
            ref: name,
            object,
          });
        }
        const names = await git.listBranches({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
        });
        return Promise.all(
          names.sort().map(async (branch) => ({
            name: branch,
            current: branch === current,
            oid: await git.resolveRef({
              fs,
              dir: repo.dir,
              gitdir: repo.gitdir,
              ref: branch,
            }),
          })),
        );
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
        await assertSupportedModes(repo);
        const committer = await localIdentity(repo);
        const oid = await git.commit({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          message: message.join("\n\n"),
          author: author ? parseIdentity(author) : committer,
          committer,
          noUpdateBranch: false,
          dryRun: false,
          disallowEmpty: !allowEmpty,
        });
        return { oid, summary: message[0] ?? "" };
      }),
    checkout: async ({ workingDirectory, newBranch, detach, ref, paths }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        await assertSupportedModes(repo);
        if (paths.length) {
          if (newBranch || detach || ref)
            throw new Error(
              "path checkout cannot be combined with branch switching",
            );
          const filepaths = relativePaths(repo, paths);
          await git.checkout({
            fs,
            dir: repo.dir,
            gitdir: repo.gitdir,
            filepaths,
            force: true,
          });
          return {
            summary: `restored ${filepaths.length} path(s) from the index`,
            paths: filepaths,
          };
        }
        if (newBranch) {
          if (detach) throw new Error("-b and --detach are mutually exclusive");
          const start = ref ? await resolveDocumentedRef(repo, ref) : undefined;
          await git.branch({
            fs,
            dir: repo.dir,
            gitdir: repo.gitdir,
            ref: newBranch,
            object: start,
          });
          try {
            await git.checkout({
              fs,
              dir: repo.dir,
              gitdir: repo.gitdir,
              ref: newBranch,
            });
          } catch (error) {
            await git.deleteBranch({
              fs,
              dir: repo.dir,
              gitdir: repo.gitdir,
              ref: newBranch,
            });
            throw error;
          }
          return {
            summary: `switched to a new branch '${newBranch}'`,
            paths: [],
          };
        }
        const target = ref;
        if (!target)
          throw new Error(
            "checkout requires a branch, --detach <ref>, or -- <paths>",
          );
        await resolveDocumentedRef(repo, target);
        await git.checkout({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          ref: target,
        });
        if (detach)
          await fs.promises.writeFile(
            path.join(repo.gitdir, "HEAD"),
            `${await resolveDocumentedRef(repo, target)}\n`,
          );
        return {
          summary: detach
            ? `HEAD is now at ${target}`
            : `switched to branch '${target}'`,
          paths: [],
        };
      }),
    config: async ({ workingDirectory, local, key, value }) =>
      guard(async () => {
        if (!local) throw new Error("only --local configuration is supported");
        if (key !== "user.name" && key !== "user.email")
          throw new Error("only user.name and user.email are supported");
        const repo = await repository(workingDirectory ?? []);
        if (value !== undefined)
          await git.setConfig({
            fs,
            dir: repo.dir,
            gitdir: repo.gitdir,
            path: key,
            value,
          });
        const configured = await git.getConfig({
          fs,
          dir: repo.dir,
          gitdir: repo.gitdir,
          path: key,
        });
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
