import { defineAgent, method } from "@golemcloud/golem-ts-sdk";
import fs from "node:fs";
import { Buffer } from "node:buffer";
import * as git from "isomorphic-git";
import { httpTransport } from "../../../builtin-tools/git/src/remote.ts";
import { wasiHttp } from "../../../builtin-tools/git/src/wasi-http.ts";
import { z } from "zod/v4";

const GitNetworkProbe = defineAgent({
  name: "GitNetworkProbe",
  id: { name: z.string() },
  methods: {
    discover: method({
      input: { url: z.string() },
      returns: z.array(z.string()),
    }),
    prepare: method({
      input: {},
      returns: z.object({ oid: z.string(), pack: z.string() }),
    }),
    transfer: method({
      input: { url: z.string(), token: z.string() },
      returns: z.object({ oid: z.string(), contents: z.string() }),
    }),
  },
});

GitNetworkProbe.implement({
  init: () => ({}),
  methods: {
    async discover({ url }) {
      const refs = await httpTransport(undefined, wasiHttp).run((http) =>
        git.listServerRefs({ http, url, protocolVersion: 1 }),
      );
      return refs.map((ref) => `${ref.ref} ${ref.oid ?? ""}`);
    },
    async prepare() {
      const dir = "/seed";
      await git.init({ fs, dir, defaultBranch: "main" });
      const blob = await git.writeBlob({
        fs,
        dir,
        blob: Buffer.from("actual smart-HTTP pack transfer\n"),
      });
      const tree = await git.writeTree({
        fs,
        dir,
        tree: [{ mode: "100644", path: "file.txt", oid: blob, type: "blob" }],
      });
      const identity = {
        name: "Transport Fixture",
        email: "fixture@example.org",
        timestamp: 1700000000,
        timezoneOffset: 0,
      };
      const oid = await git.writeCommit({
        fs,
        dir,
        commit: {
          tree,
          parent: [],
          author: identity,
          committer: identity,
          message: "fixture\n",
        },
      });
      const { packfile } = await git.packObjects({
        fs,
        dir,
        oids: [oid, tree, blob],
        write: false,
      });
      return { oid, pack: Buffer.from(packfile!).toString("base64") };
    },
    async transfer({ url, token }) {
      const dir = "/transferred";
      await httpTransport({ origin: url, bearer: token }, wasiHttp).run(
        (http) => git.clone({ fs, http, dir, url, singleBranch: true }),
      );
      return {
        oid: await git.resolveRef({ fs, dir, ref: "HEAD" }),
        contents: await fs.promises.readFile(`${dir}/file.txt`, "utf8"),
      };
    },
  },
});
