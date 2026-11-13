// The Effect fixture of the streaming fault matrix. It implements the fixture contract
// documented in integration-tests/src/fault_matrix.rs. Record `seq` of run `run` is the number
// `run * 100_000 + seq`, for `seq` in 1..=count.
import { Effect, Schema, Stream } from "effect";
import {
  Agents,
  defineAgent,
  Ids,
  method,
  WitTypes,
} from "@golemcloud/effect-golem";

const U32 = WitTypes.Uint32;
const U32Stream = WitTypes.AgentStream(U32);
const Records = Schema.Array(U32);

const record = (run: number, seq: number) => run * 100_000 + seq;
const seqs = (count: number) => Array.from({ length: count }, (_, i) => i + 1);

const decodeGate = Schema.decodeUnknownSync(Ids.PromiseId);
const encodeGate = Schema.encodeSync(Ids.PromiseId);

const collect = (input: Stream.Stream<number, unknown>) =>
  Stream.runCollect(input).pipe(
    Effect.map((records) => Array.from(records)),
    Effect.orDie,
  );

const FaultMatrixFixture = defineAgent({
  name: "FaultMatrixFixture",
  id: { name: Schema.String },
  methods: {
    create_gate: method({ input: {}, success: Ids.PromiseId }),
    produce: method({
      input: { run: U32, count: U32, pause_after: U32, gate: Ids.PromiseId },
      success: U32Stream,
    }),
    produce_then_fail: method({
      input: { run: U32, fail_after: U32 },
      success: U32Stream,
    }),
    consume: method({ input: { input: U32Stream }, success: Records }),
    consume_then_drop: method({
      input: { input: U32Stream, keep: U32 },
      success: Records,
    }),
    pipe: method({
      input: { target: Schema.String, run: U32, count: U32, keep: U32 },
      success: Records,
    }),
    read_remote: method({
      input: {
        target: Schema.String,
        run: U32,
        count: U32,
        pause_after: U32,
        gate: Ids.PromiseId,
      },
      success: Records,
    }),
  },
});

FaultMatrixFixture.implement({
  init: () => Effect.void,
  methods: () => ({
    create_gate: () =>
      Agents.Promises.create.pipe(Effect.map(decodeGate), Effect.orDie),
    produce: ({ run, count, pause_after, gate }) =>
      Effect.gen(function* () {
        const stream = Stream.fromIterable(seqs(count)).pipe(
          Stream.mapEffect((seq) =>
            seq === pause_after + 1
              ? Agents.Promises.await(encodeGate(gate)).pipe(
                  Effect.as(record(run, seq)),
                  Effect.orDie,
                )
              : Effect.succeed(record(run, seq)),
          ),
        );
        const context = yield* Effect.context<Stream.Services<typeof stream>>();
        return stream.pipe(Stream.provideContext(context));
      }),
    produce_then_fail: ({ run, fail_after }) =>
      Effect.succeed(
        Stream.fromIterable(
          seqs(fail_after).map((seq) => record(run, seq)),
        ).pipe(
          Stream.concat(Stream.fail(new Error("the fixture producer failed"))),
        ),
      ),
    consume: ({ input }) => collect(input),
    consume_then_drop: ({ input, keep }) =>
      collect(input.pipe(Stream.take(keep))),
    pipe: ({ target, run, count, keep }) =>
      Effect.gen(function* () {
        const input = Stream.fromIterable(
          seqs(count).map((seq) => record(run, seq)),
        );
        const client = yield* FaultMatrixFixture.client.get({ name: target });
        return keep >= count
          ? yield* client.consume({ input })
          : yield* client.consume_then_drop({ input, keep });
      }).pipe(Effect.scoped, Effect.orDie),
    read_remote: ({ target, run, count, pause_after, gate }) =>
      Effect.gen(function* () {
        const client = yield* FaultMatrixFixture.client.get({ name: target });
        return yield* collect(
          yield* client.produce({ run, count, pause_after, gate }),
        );
      }).pipe(Effect.scoped, Effect.orDie),
  }),
});
