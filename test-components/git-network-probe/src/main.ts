import { defineAgent, method } from "@golemcloud/golem-ts-sdk";
import * as git from "isomorphic-git";
import http from "isomorphic-git/http/web";
import { z } from "zod/v4";

const GitNetworkProbe = defineAgent({
  name: "GitNetworkProbe",
  id: { name: z.string() },
  methods: {
    discover: method({
      input: { url: z.string() },
      returns: z.array(z.string()),
    }),
  },
});

GitNetworkProbe.implement({
  init: () => ({}),
  methods: {
    async discover({ url }) {
      const refs = await git.listServerRefs({ http, url, protocolVersion: 1 });
      return refs.map((ref) => `${ref.ref} ${ref.oid ?? ""}`);
    },
  },
});
