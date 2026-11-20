import { z } from "zod";
import {
  defineAgent,
  method,
  fork,
  forkAgent,
  getSelfAgentId,
  getSelfMetadata,
  getAgentMetadata,
  generateIdempotencyKey,
} from "@golemcloud/golem-ts-sdk";

for (const snapshots of [false, true]) {
  const spec = defineAgent({
    name: snapshots ? "SnapshotIdentity" : "Identity",
    id: { name: z.string() },
    snapshotting: snapshots ? { everyNInvocations: 2 } : "default",
    methods: {
      observe: method({ input: {}, returns: z.string() }),
      status: method({ input: {}, returns: z.string() }),
      forkAt: method({ input: { cutoff: z.string() }, returns: z.string() }),
      forkSelf: method({ input: {}, returns: z.string() }),
    },
  });
  spec.implement({
    init: ({ id }) => ({
      configured: id.name,
      saved: getSelfAgentId().toString(),
      trace: [] as string[],
      snapshotIdentity: "",
      lastFork: "",
    }),
    methods: {
      observe() {
        const self = this.getId().toString();
        // Different durable calls make an incorrect historical identity diverge.
        if (self === this.saved) getSelfMetadata();
        else generateIdempotencyKey();
        this.trace.push(self);
        return self;
      },
      status() {
        const current = this.getId();
        const metadata = getSelfMetadata();
        const addressed = getAgentMetadata(current);
        return JSON.stringify({
          configured: this.configured,
          saved: this.saved,
          trace: this.trace,
          snapshotIdentity: this.snapshotIdentity,
          lastFork: this.lastFork,
          current: current.toString(),
          metadata: metadata.agentId.toString(),
          addressed: addressed?.agentId.toString(),
        });
      },
      forkAt({ cutoff }) {
        return forkAgent(this.getId(), BigInt(cutoff)).toString();
      },
      forkSelf() {
        const before = this.getId().toString();
        const result = fork();
        const after = this.getId().toString();
        this.lastFork = JSON.stringify({
          before,
          after,
          tag: result.tag,
          target: result.val.forkedAgentId.toString(),
        });
        return this.lastFork;
      },
    },
    snapshot: {
      save() {
        return new TextEncoder().encode(
          JSON.stringify({
            configured: this.configured,
            saved: this.saved,
            trace: this.trace,
            lastFork: this.lastFork,
            snapshotIdentity: this.getId().toString(),
          }),
        );
      },
      load(bytes) {
        return JSON.parse(new TextDecoder().decode(bytes)) as {
          configured: string;
          saved: string;
          trace: string[];
          snapshotIdentity: string;
          lastFork: string;
        };
      },
    },
  });
}
