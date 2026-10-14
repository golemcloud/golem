import fs from "node:fs";
import path from "node:path";
import * as git from "isomorphic-git";
import { snapshotWithBytes, treeSnapshotMap } from "./git.ts";
import type { Repository } from "./git.ts";

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
