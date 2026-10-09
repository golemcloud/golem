import { z } from "zod";
import {
  defineAgent,
  method,
  fork,
  forkAgent,
  getRawSelfAgentId,
  getSelfMetadata,
  getAgentMetadata,
  getOplogIndex,
  ParsedAgentId,
} from "@golemcloud/golem-ts-sdk";

for (const snapshots of [false, true]) {
  const spec = defineAgent({
    name: snapshots ? "SnapshotIdentity" : "Identity",
    id: { name: z.string() },
    snapshotting: snapshots ? { everyNInvocations: 2 } : "default",
    methods: {
      observe: method({ input: {}, returns: z.string() }),
      status: method({ input: {}, returns: z.string() }),
      checkpoint: method({ input: {}, returns: z.string() }),
      forkNamed: method({
        input: { name: z.string(), cutoff: z.string() },
        returns: z.void(),
      }),
      forkSelf: method({ input: {}, returns: z.string() }),
    },
  });
  spec.implement({
    init: ({ id }) => ({
      configured: id.name,
      saved: getRawSelfAgentId().value,
      trace: [] as string[],
      snapshotIdentity: "",
      lastFork: "",
    }),
    methods: {
      observe() {
        const self = this.getId().value;
        // Different durable calls make an incorrect historical identity diverge.
        if (self.includes('"parent"')) getSelfMetadata();
        else getOplogIndex();
        this.trace.push(self);
        return self;
      },
      status() {
        const current = this.getId().value;
        const metadata = getSelfMetadata();
        const addressed = getAgentMetadata({
          ...metadata.agentId,
          agentId: current,
        });
        return JSON.stringify({
          configured: this.configured,
          saved: this.saved,
          trace: this.trace,
          snapshotIdentity: this.snapshotIdentity,
          lastFork: this.lastFork,
          current,
          metadata: metadata.agentId.agentId,
          addressed: addressed?.agentId.agentId,
        });
      },
      checkpoint() {
        return getOplogIndex().toString();
      },
      forkNamed({ name, cutoff }) {
        const source = {
          ...getSelfMetadata().agentId,
          agentId: this.getId().value,
        };
        forkAgent(
          source,
          { ...source, agentId: spec.agentId({ name }).value },
          BigInt(cutoff),
        );
      },
      forkSelf() {
        const before = this.getId();
        const result = fork();
        const after = this.getId().value;
        const [typeName, constructorValue] = before.parsed();
        const target = ParsedAgentId.create({
          typeName,
          constructorValue,
          phantomId: result.val.forkedPhantomId,
        });
        this.lastFork = JSON.stringify({
          before: before.value,
          after,
          tag: result.tag,
          target: target.value,
          phantom: this.getPhantomId()?.toString(),
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
            snapshotIdentity: this.getId().value,
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
