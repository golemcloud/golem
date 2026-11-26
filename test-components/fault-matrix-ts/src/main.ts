// The TypeScript fixture of the streaming fault matrix. It implements the fixture contract
// documented in integration-tests/src/fault_matrix.rs. Record `seq` of run `run` is the number
// `run * 100_000 + seq`, for `seq` in 1..=count.
import { z } from "zod";
import {
  AgentStream,
  awaitPromise,
  createPromise,
  defineAgent,
  method,
  ok,
  s,
  toolDefinition,
  Uuid,
} from "@golemcloud/golem-ts-sdk";
import type { PromiseId } from "golem:api/host@1.5.0";

const PromiseIdSchema = z.object({
  agentId: z.object({ uuid: s.uuid() }),
  fingerprint: s.uuid(),
  oplogIdx: s.u64(),
});

const U32 = s.u32() as unknown as z.ZodType<number, number>;
const U32Stream = s.stream(U32) as unknown as z.ZodType<
  AgentStream<number>,
  AgentStream<number>
>;
const Records = z.array(U32);

const record = (run: number, seq: number) => run * 100_000 + seq;

async function collect(input: AgentStream<number>, keep = Infinity) {
  const records: number[] = [];
  while (records.length < keep) {
    const next = await input.next();
    if (next.done) return records;
    records.push(next.value);
  }
  await input.return();
  return records;
}

export const FaultMatrixFixture = defineAgent({
  name: "FaultMatrixFixture",
  id: { name: z.string() },
  methods: {
    create_gate: method({ input: {}, returns: PromiseIdSchema }),
    produce: method({
      input: { run: U32, count: U32, pause_after: U32, gate: PromiseIdSchema },
      returns: U32Stream,
    }),
    produce_then_fail: method({
      input: { run: U32, fail_after: U32 },
      returns: U32Stream,
    }),
    consume: method({ input: { input: U32Stream }, returns: Records }),
    consume_then_drop: method({
      input: { input: U32Stream, keep: U32 },
      returns: Records,
    }),
    pipe: method({
      input: { target: z.string(), run: U32, count: U32, keep: U32 },
      returns: Records,
    }),
    read_remote: method({
      input: {
        target: z.string(),
        run: U32,
        count: U32,
        pause_after: U32,
        gate: PromiseIdSchema,
      },
      returns: Records,
    }),
  },
});

export const FaultMatrixFixtureImpl = FaultMatrixFixture.implement({
  init: () => ({}),
  methods: {
    create_gate() {
      const id = createPromise();
      return {
        ...id,
        agentId: { uuid: Uuid.from(id.agentId.uuid) },
        fingerprint: Uuid.from(id.fingerprint),
      };
    },
    produce({ run, count, pause_after, gate }) {
      return AgentStream.from(
        (async function* () {
          for (let seq = 1; seq <= count; seq++) {
            if (seq === pause_after + 1) {
              await awaitPromise(gate as unknown as PromiseId);
            }
            yield record(run, seq);
          }
        })(),
      );
    },
    produce_then_fail({ run, fail_after }) {
      return AgentStream.from(
        (async function* () {
          for (let seq = 1; seq <= fail_after; seq++) {
            yield record(run, seq);
          }
          throw new Error("the fixture producer failed");
        })(),
      );
    },
    async consume({ input }) {
      return collect(input);
    },
    async consume_then_drop({ input, keep }) {
      return collect(input, keep);
    },
    async pipe({ target, run, count, keep }) {
      const input = AgentStream.from(
        (function* () {
          for (let seq = 1; seq <= count; seq++) yield record(run, seq);
        })(),
      );
      const client = FaultMatrixFixture.client.upsert({ name: target });
      return keep >= count
        ? client.consume({ input })
        : client.consume_then_drop({ input, keep });
    },
    async read_remote({ target, run, count, pause_after, gate }) {
      const output = await FaultMatrixFixture.client
        .upsert({ name: target })
        .produce({ run, count, pause_after, gate });
      return collect(output);
    },
  },
});

const recordBytes = (run: number, seq: number) => {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, record(run, seq));
  return bytes;
};

toolDefinition("fault-matrix")
  .version("1.0.0")
  .command("produce", (produce) =>
    produce.body((body) =>
      body
        .positional("run", U32)
        .positional("count", U32)
        .positional("pause-after", U32)
        .positional("gate", PromiseIdSchema)
        .stdout({ required: true })
        .returns(U32),
    ),
  )
  .implement({
    produce: async ({ run, count, pauseAfter, gate }, context) => {
      const stdout = context.stdout.getWriter();
      for (let seq = 1; seq <= count; seq++) {
        if (seq === pauseAfter + 1) {
          await awaitPromise(gate as unknown as PromiseId);
        }
        await stdout.write(recordBytes(run, seq));
      }
      await stdout.close();
      return ok(count);
    },
  });
