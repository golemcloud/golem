import fs from "node:fs";
import path from "node:path";
import * as git from "../golem-temp/isomorphic-git/index.js";
import { createXdiff } from "../golem-temp/xdiff/index.js";
import {
  MAX_DIFF_INPUT_BYTES,
  sameSnapshot,
  snapshotWithBytes,
  treeSnapshotMap,
} from "./git.ts";
import type { Repository, Snapshot } from "./git.ts";

export async function assertRebaseAttributes(
  repo: Repository,
  revisions: string[],
): Promise<void> {
  const unsupported = (content: Uint8Array, source: string) => {
    if (
      Buffer.from(content)
        .toString("utf8")
        .split(/\r?\n/)
        .some(
          (line) =>
            !/^\s*(#|$)/.test(line) &&
            /(?:^|\s)[!-]?(?:binary|diff|merge|text|crlf|eol|filter|working-tree-encoding)(?:=|\s|$)/.test(
              line,
            ),
        )
    )
      throw new Error(
        `attribute-dependent rebase is not supported: ${source}; no rebase state was created`,
      );
  };
  if (await git.getConfig({ fs, ...repo, path: "core.attributesfile" }))
    throw new Error(
      "rebase with core.attributesFile is not supported; no rebase state was created",
    );
  const attributes = new Set([
    path.join(repo.gitdir, "info/attributes"),
    path.join(repo.dir, ".gitattributes"),
  ]);
  const inspected = new Set<string>();
  for (const revision of new Set(revisions)) {
    for (const [file, snapshot] of await treeSnapshotMap(repo, revision)) {
      for (
        let parent = path.dirname(file);
        parent !== ".";
        parent = path.dirname(parent)
      )
        attributes.add(path.join(repo.dir, parent, ".gitattributes"));
      if (
        path.basename(file) !== ".gitattributes" ||
        inspected.has(snapshot.oid!)
      )
        continue;
      inspected.add(snapshot.oid!);
      unsupported((await snapshotWithBytes(repo, snapshot)).bytes!, file);
    }
  }
  for (const file of attributes) {
    try {
      unsupported(await fs.promises.readFile(file), file);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }
}

// Git's patch-id whitespace set is deliberately narrower than JavaScript's.
function stripSpace(bytes: Uint8Array): Buffer {
  return Buffer.from(bytes.filter((byte) => ![9, 10, 13, 32].includes(byte)));
}

export async function patchIds(
  repo: Repository,
  commits: string[],
  headerOnly = false,
): Promise<Map<string, string>> {
  const result = new Map<string, string>();
  if (!commits.length) return result;
  let engine: ReturnType<typeof createXdiff> | undefined;
  for (const oid of commits) {
    const { commit } = await git.readCommit({ fs, ...repo, oid });
    if (commit.parent.length > 1) continue;
    const before = commit.parent[0]
      ? await treeSnapshotMap(repo, commit.parent[0])
      : new Map<string, Snapshot>();
    const after = await treeSnapshotMap(repo, oid);
    const sum = Buffer.alloc(20);
    let changed = false;
    for (const file of new Set([...before.keys(), ...after.keys()])) {
      const old = before.get(file),
        next = after.get(file);
      if (sameSnapshot(old ?? {}, next ?? {})) continue;
      changed = true;
      if ([old?.mode, next?.mode].includes(0o160000))
        throw new Error("submodule patch equivalence is not supported");
      const filename = stripSpace(Buffer.from(file)).toString();
      let header = `diff--gita/${filename}b/${filename}`;
      if (!old) header += `newfilemode${next!.mode!.toString(8)}`;
      else if (!next) header += `deletedfilemode${old.mode!.toString(8)}`;
      else if (old.mode !== next.mode)
        header += `oldmode${old.mode!.toString(8)}newmode${next.mode!.toString(8)}`;
      const a = headerOnly
        ? new Uint8Array()
        : ((await snapshotWithBytes(repo, old)).bytes ?? new Uint8Array());
      const b = headerOnly
        ? new Uint8Array()
        : ((await snapshotWithBytes(repo, next)).bytes ?? new Uint8Array());
      let content: Uint8Array = new Uint8Array();
      if (headerOnly) {
        // Git first compares headers, calculating full IDs only on collision.
      } else if ([a, b].some((bytes) => bytes.slice(0, 8000).includes(0))) {
        content = Buffer.from(
          `${old?.oid ?? "0".repeat(40)}${next?.oid ?? "0".repeat(40)}`,
        );
      } else {
        header += old ? `---a/${filename}` : "---/dev/null";
        header += next ? `+++b/${filename}` : "+++/dev/null";
        if (a.length + b.length > MAX_DIFF_INPUT_BYTES)
          throw new Error(
            `patch equivalence exceeds the ${MAX_DIFF_INPUT_BYTES}-byte computation limit: ${file}`,
          );
        engine ??= createXdiff();
        engine.reset();
        const capacity = 2 * (a.length + b.length) + 16;
        const ap = engine.allocate(a.length),
          bp = engine.allocate(b.length);
        const out = engine.allocate(capacity);
        const memory = new Uint8Array(engine.memory.buffer);
        memory.set(a, ap);
        memory.set(b, bp);
        const length = engine.normalized_diff(
          ap,
          a.length,
          bp,
          b.length,
          out,
          capacity,
        );
        if (length < 0)
          throw new Error(`cannot calculate patch equivalence: ${file}`);
        content = memory.slice(out, out + length);
      }
      const digest = Buffer.from(
        await git.shasum(Buffer.concat([Buffer.from(header), content])),
        "hex",
      );
      let carry = 0;
      for (let i = 0; i < sum.length; i++) {
        carry += sum[i]! + digest[i]!;
        sum[i] = carry & 255;
        carry >>= 8;
      }
    }
    if (changed) result.set(oid, sum.toString("hex"));
  }
  return result;
}
