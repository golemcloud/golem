import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import {
  c,
  command,
  err,
  ok,
  s,
  toolDefinition,
  withIdempotenceMode,
} from "@golemcloud/golem-ts-sdk";
import {
  compileSchema,
  schemaGraphToWit,
  schemaValueFromWit,
} from "@golemcloud/golem-ts-sdk/schema";
import { reveal } from "golem:secrets/reveal@0.1.0";
import type { Secret } from "golem:core/types@2.0.0";
import { z } from "zod/v4";
import {
  configure,
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
  trackingForRef,
} from "./remote.js";
import type { Credentials } from "./remote.js";
import { wasiHttp } from "./wasi-http.js";
import { cloneCommand, fetchCommand } from "./transfer.js";
import { pushCommand } from "./push.js";
import {
  abortRebase,
  continueRebase,
  rebaseCommand,
  skipRebase,
} from "./rebase.js";
import {
  abortMerge,
  continueMerge,
  mergeCommand,
  pullCommand,
} from "./integration.js";
import {
  branchCommand,
  checkoutCommand,
  createCommit,
  diff,
  formatLog,
  formatStatus,
  initializeRepository,
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
    remote: z.boolean().optional(),
    upstream: z.string().optional(),
    gone: z.boolean().optional(),
    ahead: z.number().optional(),
    behind: z.number().optional(),
  }),
);

const authentication = z.object({
  origin: z.string(),
  username: z.string().optional(),
  password: z.string().optional(),
  bearer: z.string().optional(),
  headers: z.record(z.string(), z.string()).optional(),
});
const authenticationCodec = compileSchema(authentication);

function credentials(handle: Secret | undefined): Credentials | undefined {
  if (handle === undefined) return undefined;
  try {
    return authentication.parse(
      authenticationCodec.fromValue(
        schemaValueFromWit(
          reveal(handle, schemaGraphToWit(authenticationCodec.graph)),
        ),
      ),
    );
  } finally {
    (handle as unknown as { [Symbol.dispose](): void })[Symbol.dispose]();
  }
}

const definition = toolDefinition("git", { requiresFilesystem: true })
  .version("0.1.9")
  .doc({
    summary: "Run local and HTTP(S) remote Git workflows.",
    description:
      "Git commands backed by isomorphic-git and WASI HTTP. Supports remote configuration, tracking, clone, fetch, push, pull, merge conflict resolution, and persistent linear rebase. SSH, git://, subprocess credential helpers, editors, and hooks are not supplied by this runtime. The shared tool interface requires long names for short-only Git options and accepts inherited -C after a subcommand.",
    examples: [
      {
        title: "Inspect a repository",
        body: "git -C workspace status --short",
      },
      {
        title: "Stage and commit a path",
        body: 'git -C workspace add -- src/main.ts\ngit -C workspace commit -m "Update main"',
      },
    ],
  })
  .global("authentication", s.secret(authentication), {
    doc: "Opaque, invocation-scoped HTTP credentials bound to one origin. Never stored in Git config.",
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
          .flag("null", {
            short: "z",
            doc: "Terminate output records with NUL.",
          })
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
          "With no references, compares the index to the worktree. Unmerged paths are marked explicitly and compared against stage 2 (ours), not rendered as combined diffs. --cached compares HEAD to the index and identifies unmerged paths. One reference compares that tree to the worktree; two compare tree to tree. Output limits fail explicitly rather than truncate patches.",
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
          .flag("remotes", {
            short: "r",
            doc: "List cached remote-tracking branches.",
          })
          .flag("all", {
            short: "a",
            doc: "List local and cached remote-tracking branches.",
          })
          .flag("verbose", {
            kind: "count-flag",
            short: "v",
            max: 2,
            doc: "Include upstream and ahead/behind information with -vv.",
          })
          .option("set-upstream-to", z.string(), {
            short: "u",
            doc: "Set the branch's upstream.",
          })
          .flag("unset-upstream", { doc: "Remove the branch's upstream." })
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
          .flag("all", {
            short: "A",
            doc: "Stage all additions, changes, and deletions.",
          })
          .flag("update", {
            short: "u",
            doc: "Stage changes and deletions to tracked paths.",
          })
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
          .flag("allow-empty", {
            doc: "Create a commit even when the tree is unchanged.",
          })
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
        summary: "Read or update repository-local Git settings.",
        description:
          "Supports identity, remotes, tracking, and fetch/push/pull/merge settings. Authentication is supplied through an opaque invocation secret, not Git config.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: true,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("local", {
            default: true,
            doc: "Use repository-local configuration.",
          })
          .flag("add", { doc: "Append a value to a multi-valued key." })
          .flag("get-all", { doc: "Read every value for the key." })
          .flag("unset", { doc: "Remove a single-valued key." })
          .flag("unset-all", { doc: "Remove every value for the key." })
          .positional("key", z.string(), {
            valueName: "KEY",
            doc: "Supported repository-local configuration key.",
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
              values: z.array(z.string()).optional(),
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
  .command("remote", (remote) =>
    remote
      .doc({ summary: "Manage repository-local remotes." })
      .body((body) =>
        body
          .flag("verbose", { short: "v", doc: "List fetch and push URLs." })
          .returns(z.string())
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      )
      .command("add", (node) =>
        node.body((body) =>
          body
            .positional("name", z.string())
            .positional("url", z.string())
            .returns(mutation)
            .error("git-error", {
              kind: "runtime",
              exitCode: 1,
              payload: failure,
            }),
        ),
      )
      .command("remove", (node) =>
        node.body((body) =>
          body
            .positional("name", z.string())
            .returns(mutation)
            .error("git-error", {
              kind: "runtime",
              exitCode: 1,
              payload: failure,
            }),
        ),
      )
      .command("rename", (node) =>
        node.body((body) =>
          body
            .positional("name", z.string())
            .positional("new-name", z.string())
            .returns(mutation)
            .error("git-error", {
              kind: "runtime",
              exitCode: 1,
              payload: failure,
            }),
        ),
      )
      .command("get-url", (node) =>
        node.body((body) =>
          body
            .flag("push", { doc: "Read push URLs instead of fetch URLs." })
            .flag("all", { doc: "Read every configured URL." })
            .positional("name", z.string())
            .returns(z.array(z.string()))
            .error("git-error", {
              kind: "runtime",
              exitCode: 1,
              payload: failure,
            }),
        ),
      )
      .command("set-url", (node) =>
        node.body((body) =>
          body
            .flag("push", { doc: "Modify push URLs." })
            .flag("add", { doc: "Append a URL." })
            .flag("delete", {
              doc: "Remove URLs matching the supplied pattern.",
            })
            .positional("name", z.string())
            .positional("url", z.string())
            .positional("old-url", z.string(), { required: false })
            .returns(mutation)
            .error("git-error", {
              kind: "runtime",
              exitCode: 1,
              payload: failure,
            }),
        ),
      ),
  )
  .command("ls-remote", (node) =>
    node
      .doc({ summary: "Discover remote references over HTTP(S)." })
      .annotations({
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: true,
      })
      .body((body) =>
        body
          .flag("heads", { doc: "List branches." })
          .flag("tags", { doc: "List tags, including peeled annotated tags." })
          .flag("symref", { doc: "Include symbolic reference targets." })
          .positional("repository", z.string(), { required: false })
          .returns(
            z.object({
              refs: z.array(
                z.object({
                  ref: z.string(),
                  oid: z.string(),
                  target: z.string().optional(),
                  peeled: z.string().optional(),
                }),
              ),
              stdout: z.string(),
            }),
          )
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("push", (node) =>
    node
      .doc({
        summary: "Push one branch or tag through released isomorphic-git.",
        description:
          "Multi-ref, matching and atomic pushes are unsupported and fail before sending. --all and --tags work only when selecting at most one ref. Deletion requires an existing local HEAD. A lost response has an uncertain remote outcome; reconcile before retrying. Released upstream parsing tolerates some malformed trailing status records and can wait for an unfinished response.",
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: true,
      })
      .body((body) =>
        body
          .flag("set-upstream", {
            short: "u",
            doc: "Set upstream configuration for successful branch updates.",
          })
          .flag("delete", {
            short: "d",
            doc: "Delete the named remote references.",
          })
          .flag("tags", {
            doc: "Push local tags; rejects selections containing more than one ref.",
          })
          .flag("all", {
            doc: "Push local branches; rejects selections containing more than one ref.",
          })
          .flag("force", {
            short: "f",
            doc: "Allow non-fast-forward updates and override lease expectations, like Git. A +refspec forces only that reference.",
          })
          .flag("force-with-lease", {
            doc: "Force only when the advertised remote OID matches the captured tracking OID or explicit lease.",
          })
          .option("lease", z.string(), {
            doc: "Explicit force-with-lease expectation: refs/heads/BRANCH:OID, or refs/heads/BRANCH: for absence. Requires --force-with-lease.",
          })
          .flag("atomic", {
            doc: "Unsupported by the released library; always rejects without sending.",
          })
          .positional("remote", z.string(), {
            required: false,
            valueName: "REMOTE",
          })
          .tail("refspecs", z.string(), { valueName: "REFSPEC" })
          .returns(
            z.object({
              summary: z.string(),
              refs: z.array(
                z.object({
                  url: z.string(),
                  ref: z.string(),
                  oid: z.string(),
                  oldoid: z.string(),
                  ok: z.boolean(),
                  error: z.string().optional(),
                  upToDate: z.boolean(),
                }),
              ),
            }),
          )
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("clone", (node) =>
    node
      .doc({
        summary: "Clone an HTTP(S) repository into a new or empty directory.",
        description:
          "Uses released isomorphic-git with isolated per-ref downloads. An empty remote uses master without an upstream because its unborn branch is not discoverable. Executable-file worktrees fail closed; no mode persistence workaround is supplied. Servers without sideband upload-pack can stall in the released parser.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: false,
        openWorld: true,
      })
      .body((body) =>
        body
          .option("origin", z.string(), {
            short: "o",
            doc: "Remote name; defaults to origin.",
          })
          .option("branch", z.string(), {
            short: "b",
            doc: "Initial branch or tag.",
          })
          .flag("no-checkout", {
            short: "n",
            doc: "Do not populate the worktree.",
          })
          .flag("single-branch", {
            doc: "Fetch only the selected branch; implied by depth.",
          })
          .flag("no-single-branch", {
            doc: "Fetch all branches even with depth.",
          })
          .option("depth", z.number().int().positive(), {
            doc: "Fetch this many commits of history.",
          })
          .flag("tags", { doc: "Fetch all tags." })
          .flag("no-tags", {
            doc: "Disable automatic tag following and persist that setting.",
          })
          .positional("repository", z.string(), { valueName: "REPOSITORY" })
          .positional("directory", z.string(), {
            required: false,
            valueName: "DIRECTORY",
          })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("fetch", (node) =>
    node
      .doc({
        summary:
          "Fetch validated objects and update mapped references and FETCH_HEAD.",
        description:
          "Copies existing objects to a disposable repository and fetches selected refs separately. Validates reachable objects before publication. Unsolicited shallow lines without a depth request are ignored. Raw non-sideband upload-pack can stall; partial-download replay and active cancellation still depend on unresolved runtime work.",
      })
      .annotations({
        readOnly: false,
        destructive: false,
        idempotent: false,
        openWorld: true,
      })
      .body((body) =>
        body
          .flag("all", { doc: "Fetch every configured remote." })
          .flag("prune", {
            short: "p",
            doc: "Remove stale references covered by fetch refspecs.",
          })
          .flag("no-prune", { doc: "Override configured pruning." })
          .flag("tags", { short: "t", doc: "Fetch all tags." })
          .flag("no-tags", { doc: "Do not automatically follow tags." })
          .flag("force", {
            short: "f",
            doc: "Allow non-fast-forward and changed tag updates.",
          })
          .option("depth", z.number().int().positive(), {
            doc: "Set absolute shallow depth.",
          })
          .option("deepen", z.number().int().positive(), {
            doc: "Extend existing shallow history.",
          })
          .flag("unshallow", { doc: "Fetch complete history." })
          .flag("append", {
            short: "a",
            doc: "Append rather than replace FETCH_HEAD.",
          })
          .flag("no-write-fetch-head", { doc: "Leave FETCH_HEAD untouched." })
          .positional("remote", z.string(), {
            required: false,
            valueName: "REMOTE",
          })
          .tail("refspecs", z.string(), { valueName: "REFSPEC" })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("merge", (node) =>
    node
      .doc({
        summary:
          "Integrate a revision, or continue or abort a conflicted merge.",
        description:
          "Supports fast-forward and three-way merges with one merge base. Rename-sensitive integrations, multiple merge bases, unrelated histories, and submodules fail explicitly before index/worktree publication. Resolve conflicts with add followed by commit or merge --continue.",
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("ff", { doc: "Fast-forward when possible, otherwise merge." })
          .flag("ff-only", { doc: "Reject a divergent integration." })
          .flag("no-ff", {
            doc: "Create a merge commit even when fast-forward is possible.",
          })
          .option("message", z.string(), {
            short: "m",
            doc: "Merge commit message.",
          })
          .flag("continue", { doc: "Commit the resolved merge index." })
          .flag("abort", {
            doc: "Reset the index and affected paths to pre-merge HEAD. Unrelated dirty files are preserved; unmerged-path edits may be discarded, as with Git.",
          })
          .positional("ref", z.string(), {
            required: false,
            valueName: "REVISION",
          })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("rebase", (node) =>
    node
      .doc({
        summary:
          "Replay local commits on an upstream, or continue, skip or abort a stopped replay.",
        description:
          "Supports linear commit replay using released JavaScript diff3. Unlike Git, no patch-ID filtering is performed: whitespace-equivalent upstream changes can stop with conflicts. Non-empty commits whose replay leaves the tree unchanged are dropped; originally empty commits are preserved. Rename-sensitive replay can stop after rebase has started; use --abort to restore the original branch. Merge-commit histories and attribute-dependent diff, merge, filtering or encoding are rejected explicitly before replay.",
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      })
      .body((body) =>
        body
          .flag("continue", {
            doc: "Commit the resolved index and resume replay.",
          })
          .flag("skip", {
            doc: "Discard the stopped commit's changes and resume replay.",
          })
          .flag("abort", {
            doc: "Restore the original branch, index and tracked worktree.",
          })
          .positional("upstream", z.string(), {
            required: false,
            valueName: "UPSTREAM",
          })
          .positional("branch", z.string(), {
            required: false,
            valueName: "BRANCH",
          })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .command("pull", (node) =>
    node
      .doc({
        summary:
          "Fetch and integrate the configured upstream or explicit remote branch.",
      })
      .annotations({
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: true,
      })
      .body((body) =>
        body
          .flag("ff", { doc: "Fast-forward when possible, otherwise merge." })
          .flag("ff-only", { doc: "Only fast-forward." })
          .flag("no-ff", { doc: "Always make a merge commit." })
          .flag("rebase", {
            short: "r",
            doc: "Replay local commits on the fetched upstream.",
          })
          .flag("no-rebase", { doc: "Merge instead of rebasing." })
          .positional("remote", z.string(), {
            required: false,
            valueName: "REMOTE",
          })
          .tail("refspecs", z.string(), { valueName: "REFSPEC" })
          .returns(mutation)
          .error("git-error", {
            kind: "runtime",
            exitCode: 1,
            payload: failure,
          }),
      ),
  )
  .implement({
    rebase: async ({
      workingDirectory,
      upstream,
      branch,
      continue: continuing,
      skip,
      abort,
    }) =>
      guard(async () => {
        if ([continuing, skip, abort].filter(Boolean).length > 1)
          throw new Error("rebase lifecycle options are mutually exclusive");
        if ((continuing || skip || abort) && (upstream || branch))
          throw new Error("rebase lifecycle options do not accept revisions");
        const repo = await repository(workingDirectory ?? []);
        if (continuing) return continueRebase(repo);
        if (skip) return skipRebase(repo);
        if (abort) return abortRebase(repo);
        return rebaseCommand(repo, upstream, branch);
      }),
    merge: async ({
      workingDirectory,
      ref,
      ff,
      ffOnly,
      noFf,
      message,
      continue: continuing,
      abort,
    }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        if ((continuing || abort) && (ref || ff || ffOnly || noFf || message))
          throw new Error(
            "merge lifecycle options do not accept a revision or merge options",
          );
        if (continuing && abort)
          throw new Error("--continue and --abort are mutually exclusive");
        if (abort) return abortMerge(repo);
        if (continuing)
          return {
            summary: `Merge committed ${await continueMerge(repo)}`,
            paths: [],
          };
        return mergeCommand(repo, ref, { ff, ffOnly, noFf, message });
      }),
    pull: async ({
      workingDirectory,
      remote,
      refspecs,
      ff,
      ffOnly,
      noFf,
      rebase,
      noRebase,
      authentication: handle,
    }) =>
      withAuthentication(handle, async (auth) => {
        if (rebase && noRebase)
          throw new Error("--rebase and --no-rebase are mutually exclusive");
        return pullCommand(
          await repository(workingDirectory ?? []),
          remote,
          refspecs,
          {
            ff,
            ffOnly,
            noFf,
            rebase: rebase ? true : noRebase ? false : undefined,
          },
          wasiHttp,
          auth,
        );
      }),
    push: async ({
      workingDirectory,
      remote,
      refspecs,
      setUpstream,
      delete: deleting,
      tags,
      all,
      force,
      forceWithLease,
      lease,
      atomic,
      authentication: handle,
    }) =>
      // A lost receive-pack response may follow a successful remote update.
      withIdempotenceMode(false, () =>
        withAuthentication(handle, async (auth) =>
          pushCommand(
            await repository(workingDirectory ?? []),
            remote,
            refspecs,
            {
              setUpstream,
              delete: deleting,
              tags,
              all,
              force,
              forceWithLease,
              lease,
              atomic,
            },
            wasiHttp,
            auth,
          ),
        ),
      ),
    clone: async ({
      workingDirectory,
      repository: url,
      directory,
      origin,
      branch,
      noCheckout,
      singleBranch,
      noSingleBranch,
      depth,
      tags,
      noTags,
      authentication: handle,
    }) =>
      withAuthentication(handle, async (auth) => {
        if (tags && noTags)
          throw new Error("--tags and --no-tags are mutually exclusive");
        if (singleBranch && noSingleBranch)
          throw new Error("single-branch options are mutually exclusive");
        return cloneCommand(
          await validatedCwd(workingDirectory ?? []),
          url,
          directory,
          {
            origin,
            branch,
            noCheckout,
            singleBranch: singleBranch
              ? true
              : noSingleBranch
                ? false
                : undefined,
            depth,
            tags: tags ? true : noTags ? false : undefined,
          },
          wasiHttp,
          auth,
        );
      }),
    fetch: async ({
      workingDirectory,
      remote,
      refspecs,
      all,
      prune,
      noPrune,
      tags,
      noTags,
      force,
      depth,
      deepen,
      unshallow,
      append,
      noWriteFetchHead,
      authentication: handle,
    }) =>
      withAuthentication(handle, async (auth) => {
        if (tags && noTags)
          throw new Error("--tags and --no-tags are mutually exclusive");
        if (prune && noPrune)
          throw new Error("prune options are mutually exclusive");
        return fetchCommand(
          await repository(workingDirectory ?? []),
          remote,
          refspecs,
          {
            all,
            prune: prune ? true : noPrune ? false : undefined,
            tags: tags ? true : noTags ? false : undefined,
            force,
            depth,
            deepen,
            unshallow,
            append,
            writeFetchHead: !noWriteFetchHead,
          },
          wasiHttp,
          auth,
        );
      }),
    remote: command(
      async ({ workingDirectory, verbose }) =>
        guard(async () =>
          remoteList(await repository(workingDirectory ?? []), verbose),
        ),
      {
        add: async ({ workingDirectory, name, url }) =>
          guard(async () => {
            await remoteAdd(
              await repository(workingDirectory ?? []),
              name,
              url,
            );
            return { summary: `added remote ${name}`, paths: [] };
          }),
        remove: async ({ workingDirectory, name }) =>
          guard(async () => {
            await remoteRenameOrRemove(
              await repository(workingDirectory ?? []),
              name,
            );
            return { summary: `removed remote ${name}`, paths: [] };
          }),
        rename: async ({ workingDirectory, name, newName }) =>
          guard(async () => {
            await remoteRenameOrRemove(
              await repository(workingDirectory ?? []),
              name,
              newName,
            );
            return {
              summary: `renamed remote ${name} to ${newName}`,
              paths: [],
            };
          }),
        "get-url": async ({ workingDirectory, name, push, all }) =>
          guard(async () => {
            const urls = await remoteUrls(
              await repository(workingDirectory ?? []),
              name,
              push,
            );
            return (all ? urls : urls.slice(0, 1)).map((url) => redact(url));
          }),
        "set-url": async ({
          workingDirectory,
          name,
          url,
          push,
          add,
          delete: remove,
          oldUrl,
        }) =>
          guard(async () => {
            await remoteSetUrl(
              await repository(workingDirectory ?? []),
              name,
              url,
              push,
              add,
              remove,
              oldUrl,
            );
            return { summary: `updated remote ${name}`, paths: [] };
          }),
      },
    ),
    "ls-remote": async ({
      workingDirectory,
      repository: remote,
      heads,
      tags,
      symref,
      authentication: handle,
    }) => {
      let auth: Credentials | undefined;
      try {
        auth = credentials(handle);
      } catch {
        return err(
          "git-error",
          "authentication secret could not be decoded or revealed",
        );
      }
      return guard(async () => {
        const cwd = await validatedCwd(workingDirectory ?? []);
        let repo;
        try {
          repo = await repository([cwd]);
        } catch (error) {
          if ((error as { code?: string }).code !== "NotFoundError")
            throw error;
        }
        return lsRemote(repo, remote, heads, tags, symref, wasiHttp, auth);
      }, auth);
    },
    init: async ({ workingDirectory, directory, initialBranch }) =>
      guard(async () => {
        const dir = path.resolve(
          await validatedCwd(workingDirectory ?? []),
          directory ?? ".",
        );
        await initializeRepository(dir, initialBranch ?? "main");
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
      remotes,
      all,
      verbose,
      setUpstreamTo,
      unsetUpstream,
    }) =>
      guard(async () => {
        const repo = await repository(workingDirectory ?? []);
        if (setUpstreamTo !== undefined || unsetUpstream) {
          if (
            (setUpstreamTo !== undefined && unsetUpstream) ||
            shouldDelete ||
            startPoint ||
            remotes ||
            all
          )
            throw new Error(
              "upstream actions cannot be combined with branch creation, deletion, or remote listing",
            );
          const branch = name ?? (await git.currentBranch({ fs, ...repo }));
          if (!branch) throw new Error("HEAD is detached");
          await setUpstream(repo, branch, setUpstreamTo);
          return trackedBranches(repo, false, false, 2);
        }
        if (remotes || all || verbose) {
          if (name || startPoint || shouldDelete)
            throw new Error(
              "listing options cannot be combined with branch mutation",
            );
          return trackedBranches(repo, remotes, all, verbose);
        }
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
        const tracking =
          ref && !newBranch && !detach && !paths.length
            ? await trackingCheckoutTarget(repo, ref)
            : undefined;
        const explicitTracking =
          newBranch && ref && !detach && !paths.length
            ? await trackingForRef(repo, ref)
            : undefined;
        const result = await checkoutCommand(
          repo,
          tracking ? ref : newBranch,
          detach,
          tracking?.ref ?? ref,
          paths,
        );
        if (tracking) await setUpstream(repo, ref!, tracking);
        else if (
          newBranch &&
          explicitTracking?.remote !== "." &&
          explicitTracking
        )
          await setUpstream(repo, newBranch, explicitTracking);
        return result;
      }),
    config: async ({
      workingDirectory,
      local,
      key,
      value,
      add,
      getAll,
      unset,
      unsetAll,
    }) =>
      guard(async () => {
        if (!local) throw new Error("only --local configuration is supported");
        const repo = await repository(workingDirectory ?? []);
        return configure(repo, key, value, { add, getAll, unset, unsetAll });
      }),
  });

async function withAuthentication<T>(
  handle: Secret | undefined,
  operation: (auth: Credentials | undefined) => Promise<T>,
) {
  let auth: Credentials | undefined;
  try {
    auth = credentials(handle);
  } catch {
    return err(
      "git-error",
      "authentication secret could not be decoded or revealed",
    );
  }
  return guard(() => operation(auth), auth);
}

async function guard<T>(operation: () => Promise<T>, auth?: Credentials) {
  try {
    return ok(await operation());
  } catch (error) {
    return err(
      "git-error",
      redact(error instanceof Error ? error.message : String(error), auth),
    );
  }
}

export { definition };
