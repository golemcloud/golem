import { z } from "zod";
import {
  AgentStream,
  defineAgent,
  method,
  s,
  clientFor,
  createPromise,
  awaitPromise,
} from "@golemcloud/golem-ts-sdk";
import type { PromiseId } from "golem:api/host@1.5.0";
import * as process from "node:process";

const EnvVar = z.object({ key: z.string(), value: z.string() });

// A `PromiseId` is a nested host record carrying bigints; declare it as an
// explicit Standard Schema so it can be returned by a method.
const PromiseIdSchema = z.object({
  agentId: z.object({
    componentId: z.object({
      uuid: z.object({ highBits: s.u64(), lowBits: s.u64() }),
    }),
    agentId: z.string(),
  }),
  oplogIdx: s.u64(),
});

export const ChildAgent = defineAgent({
  name: "ChildAgent",
  id: { id: z.number() },
  methods: {
    process: method({ input: {}, returns: z.number() }),
    envVars: method({ input: {}, returns: z.array(EnvVar) }),
    longRpcCall: method({
      input: { durationInMillis: z.number() },
      returns: z.void(),
    }),
  },
});

const childClient = clientFor(ChildAgent);

export const ChildAgentImpl = ChildAgent.implement({
  init: ({ id }) => ({ id: id.id }),
  methods: {
    async process() {
      const sleepAmount = Math.random() * 1000 + 500;
      await sleep(sleepAmount);
      return this.id;
    },
    envVars() {
      return Object.entries(process.env).map(([key, value]) => ({
        key,
        value: value ?? "",
      }));
    },
    async longRpcCall({ durationInMillis }) {
      console.log(`Starting sleeping ${durationInMillis}ms`);
      await sleep(durationInMillis);
      console.log(`Finished sleeping ${durationInMillis}ms`);
    },
  },
});

export const TestAgent = defineAgent({
  name: "TestAgent",
  id: { id: z.string() },
  methods: {
    run: method({ input: { n: z.number() }, returns: z.array(z.number()) }),
    envVarTest: method({
      input: {},
      returns: z.object({ parent: z.array(EnvVar), child: z.array(EnvVar) }),
    }),
    longRpcCall: method({
      input: { durationInMillis: z.number() },
      returns: z.void(),
    }),
  },
});

export const TestAgentImpl = TestAgent.implement({
  init: ({ id }) => ({ id: id.id }),
  methods: {
    async run({ n }) {
      const ids = Array.from({ length: n }, (_, i) => i);
      const chunks = arrayChunks(ids, 5);

      const result: number[] = [];
      for (const chunk of chunks) {
        console.log(`Processing chunk ${chunk}`);
        const promises = chunk.map(
          async (id) => await childClient({ id }).process(),
        );
        result.push(...(await Promise.all(promises)));
      }
      return result;
    },
    async envVarTest() {
      const child = await childClient({ id: 0 }).envVars();
      const parent = Object.entries(process.env).map(([key, value]) => ({
        key,
        value: value ?? "",
      }));
      return {
        parent,
        child,
      };
    },
    async longRpcCall({ durationInMillis }) {
      await childClient({ id: 1000 }).longRpcCall({ durationInMillis });
    },
  },
});

export const SimpleChildAgent = defineAgent({
  name: "SimpleChildAgent",
  id: { name: z.string() },
  methods: {
    value: method({ input: {}, returns: z.number() }),
  },
});

export const SimpleChildAgentImpl = SimpleChildAgent.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    async value() {
      return 1;
    },
  },
});

export const SelfRpcAgent = defineAgent({
  name: "SelfRpcAgent",
  id: { name: z.string() },
  methods: {
    doWork: method({ input: {}, returns: z.void() }),
    selfRpc: method({ input: {}, returns: z.void() }),
  },
});

const selfRpcClient = clientFor(SelfRpcAgent);

export const SelfRpcAgentImpl = SelfRpcAgent.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    async doWork() {
      return;
    },
    async selfRpc() {
      return selfRpcClient({ name: this.name }).doWork();
    },
  },
});

export const TsCounter = defineAgent({
  name: "TsCounter",
  id: { name: z.string() },
  methods: {
    incBy: method({ input: { value: z.number() }, returns: z.void() }),
    getValue: method({ input: {}, returns: z.number() }),
    slowIncBy: method({
      input: { value: z.number(), delayMs: z.number() },
      returns: z.void(),
    }),
  },
});

export const TsCounterImpl = TsCounter.implement({
  init: () => ({ count: 0 }),
  methods: {
    incBy({ value }) {
      this.count += value;
    },
    getValue() {
      return this.count;
    },
    async slowIncBy({ value, delayMs }) {
      await sleep(delayMs);
      this.count += value;
    },
  },
});

export const TsBlockingAgent = defineAgent({
  name: "TsBlockingAgent",
  id: { name: z.string() },
  methods: {
    prepareBlock: method({ input: {}, returns: PromiseIdSchema }),
    doBlock: method({ input: {}, returns: z.number() }),
    getCompletedCount: method({ input: {}, returns: z.number() }),
  },
});

export const TsBlockingAgentImpl = TsBlockingAgent.implement({
  init: () => ({
    storedPromiseId: undefined as PromiseId | undefined,
    completedCount: 0,
  }),
  methods: {
    prepareBlock() {
      const id = createPromise();
      this.storedPromiseId = id;
      return id;
    },
    async doBlock() {
      if (!this.storedPromiseId) {
        throw new Error("prepareBlock() must be called first");
      }
      await awaitPromise(this.storedPromiseId);
      this.completedCount += 1;
      return this.completedCount;
    },
    getCompletedCount() {
      return this.completedCount;
    },
  },
});

const tsCounterClient = clientFor(TsCounter);
const tsBlockingClient = clientFor(TsBlockingAgent);

export const TsCancelTester = defineAgent({
  name: "TsCancelTester",
  id: { name: z.string() },
  methods: {
    testAbortBeforeAwait: method({
      input: { counterName: z.string() },
      returns: z.string(),
    }),
    testAbortAfterComplete: method({
      input: { counterName: z.string() },
      returns: z.number(),
    }),
  },
});

export const TsCancelTesterImpl = TsCancelTester.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    /**
     * Starts an abortable RPC call to TsCounter.slowIncBy, aborts after a
     * short delay, and returns "aborted" if the AbortError is caught.
     */
    async testAbortBeforeAwait({ counterName }) {
      const counter = tsCounterClient({ name: counterName });
      const controller = new AbortController();

      // Abort after 100ms — slowIncBy takes 5000ms so it is still pending.
      setTimeout(() => controller.abort("cancelled by test"), 100);

      try {
        await counter.slowIncBy(
          { value: 1, delayMs: 5000 },
          { signal: controller.signal },
        );
        return "unexpected:completed";
      } catch (e: any) {
        if (e === "cancelled by test" || e?.name === "AbortError") {
          return "aborted";
        }
        return `unexpected:error:${String(e)}`;
      }
    },

    /**
     * Starts an abortable RPC call, awaits completion, then aborts (a no-op).
     * Returns the counter value.
     */
    async testAbortAfterComplete({ counterName }) {
      const counter = tsCounterClient({ name: counterName });
      const controller = new AbortController();

      // Completes quickly.
      await counter.incBy({ value: 5 }, { signal: controller.signal });

      // Abort after completion — a no-op.
      controller.abort("late abort");

      return await counter.getValue();
    },
  },
});

export const TsCancelCallerAgent = defineAgent({
  name: "TsCancelCallerAgent",
  id: { name: z.string() },
  methods: {
    callAndAbort: method({
      input: { targetName: z.string(), delayMs: z.number() },
      returns: z.string(),
    }),
    getLastOutcome: method({ input: {}, returns: z.string() }),
  },
});

export const TsCancelCallerAgentImpl = TsCancelCallerAgent.implement({
  init: ({ id }) => ({ name: id.name, lastOutcome: "none" }),
  methods: {
    async callAndAbort({ targetName, delayMs }) {
      const blocker = tsBlockingClient({ name: targetName });
      const controller = new AbortController();

      const timer = setTimeout(
        () => controller.abort("cancelled by test"),
        delayMs,
      );

      try {
        await blocker.doBlock({ signal: controller.signal });
        this.lastOutcome = "unexpected:completed";
      } catch (e: any) {
        if (e === "cancelled by test" || e?.name === "AbortError") {
          this.lastOutcome = "aborted";
        } else {
          this.lastOutcome = `unexpected:error:${String(e)}`;
        }
      } finally {
        clearTimeout(timer);
      }
      return this.lastOutcome;
    },
    getLastOutcome() {
      return this.lastOutcome;
    },
  },
});

const U32 = s.u32() as unknown as z.ZodType<number, number>;
const U32Stream = s.stream(U32) as unknown as z.ZodType<
  AgentStream<number>,
  AgentStream<number>
>;
const StringStream = s.stream(z.string()) as unknown as z.ZodType<
  AgentStream<string>,
  AgentStream<string>
>;
const U32List = z.array(U32);
const NestedStreamInput = z.object({
  labels: StringStream,
  values: U32Stream,
});
const NestedStreamItem = z.object({
  label: z.string(),
  values: U32Stream,
}) as z.ZodType<
  { label: string; values: AgentStream<number> },
  { label: string; values: AgentStream<number> }
>;
const NestedStreamItemStream = s.stream<{
  label: string;
  values: AgentStream<number>;
}>(NestedStreamItem);
const StreamingRpcReport = z.object({
  inputOnly: U32List,
  outputOnly: U32List,
  simultaneous: U32List,
  forwarded: U32List,
  nestedLabels: z.array(z.string()),
  nestedValues: U32List,
  nestedItemLabels: z.array(z.string()),
  nestedItemValues: z.array(U32List),
  firstSibling: z.array(z.string()),
  secondSibling: U32List,
  outputFirst: U32,
  afterConsumerReturn: U32,
});

export const TsStreamingRpcTarget = defineAgent({
  name: "TsStreamingRpcTarget",
  id: { name: z.string() },
  methods: {
    consume: method({
      input: { input: s.stream(s.u32()) },
      returns: U32List,
    }),
    consumeFirst: method({
      input: { input: s.stream(s.u32()) },
      returns: s.u32(),
    }),
    produce: method({
      input: { values: U32List },
      returns: s.stream(s.u32()),
    }),
    transform: method({
      input: { input: s.stream(s.u32()) },
      returns: s.stream(s.u32()),
    }),
    forward: method({
      input: { input: s.stream(s.u32()) },
      returns: s.stream(s.u32()),
    }),
    consumeNested: method({
      input: { input: NestedStreamInput },
      returns: z.tuple([z.array(z.string()), U32List]),
    }),
    produceNestedItems: method({
      input: {},
      returns: NestedStreamItemStream,
    }),
    produceSiblings: method({
      input: {},
      returns: z.tuple([StringStream, U32Stream]),
    }),
    produceError: method({ input: {}, returns: s.stream(s.u32()) }),
    ping: method({ input: {}, returns: s.u32() }),
    incrementScalar: method({ input: {}, returns: s.u32() }),
  },
});

export const TsStreamingRpcTargetImpl = TsStreamingRpcTarget.implement({
  init: () => ({ scalar: 0 }),
  methods: {
    async consume({ input }) {
      return collect(input);
    },
    async consumeFirst({ input }) {
      const first = await input.next();
      await input.return();
      if (first.done) {
        throw new Error("expected at least one input stream value");
      }
      return first.value;
    },
    produce({ values }) {
      return AgentStream.from(values);
    },
    transform({ input }) {
      return AgentStream.from(
        (async function* () {
          for await (const value of input) {
            yield value * 10;
          }
        })(),
      );
    },
    forward({ input }) {
      return input;
    },
    async consumeNested({ input }) {
      return [
        await collect(input.labels),
        await collect(input.values),
      ] as const;
    },
    produceNestedItems() {
      return AgentStream.from([
        { label: "first", values: AgentStream.from([1, 2]) },
        { label: "second", values: AgentStream.from([3, 4, 5]) },
      ]);
    },
    produceSiblings() {
      return [
        AgentStream.from(["a", "b"]),
        AgentStream.from(Array.from({ length: 64 }, (_, value) => value)),
      ] as const;
    },
    produceError() {
      return AgentStream.from(
        (async function* () {
          yield 1;
          throw new Error("ts-producer-failed");
        })(),
      );
    },
    ping() {
      return 42;
    },
    incrementScalar() {
      this.scalar += 1;
      return this.scalar;
    },
  },
});

const tsStreamingTargetClient = clientFor(TsStreamingRpcTarget);

export const TsStreamingRpcCaller = defineAgent({
  name: "TsStreamingRpcCaller",
  id: { name: z.string() },
  methods: {
    run: method({ input: {}, returns: StreamingRpcReport }),
    callProducerError: method({ input: {}, returns: U32List }),
    callStreamFree: method({ input: {}, returns: s.u32() }),
  },
});

export const TsStreamingRpcCallerImpl = TsStreamingRpcCaller.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    async run() {
      const target = tsStreamingTargetClient({ name: this.name });

      const inputOnly = await target.consume({
        input: AgentStream.from([1, 2, 3]),
      });
      const outputOnly = await collect(
        await target.produce({ values: [4, 5, 6] }),
      );
      const simultaneous = await collect(
        await target.transform({ input: AgentStream.from([7, 8, 9]) }),
      );
      const forwarded = await collect(
        await target.forward({ input: AgentStream.from([12, 13, 14]) }),
      );
      const [nestedLabels, nestedValues] = await target.consumeNested({
        input: {
          labels: AgentStream.from(["left", "right"]),
          values: AgentStream.from([10, 11]),
        },
      });

      const nestedItemLabels: string[] = [];
      const nestedItemValues: number[][] = [];
      for await (const item of await target.produceNestedItems()) {
        nestedItemLabels.push(item.label);
        nestedItemValues.push(await collect(item.values));
      }

      const [first, second] = await target.produceSiblings();
      const firstSibling = await collect(first);
      const secondSibling = await collect(second);

      const consumedFirst = await target.consumeFirst({
        input: AgentStream.from([30, 31, 32]),
      });
      if (consumedFirst !== 30) {
        throw new Error(`expected first input value, got ${consumedFirst}`);
      }

      const cancellableOutput = await target.produce({
        values: [100, 101, 102],
      });
      const outputFirstResult = await cancellableOutput.next();
      if (outputFirstResult.done) {
        throw new Error("expected at least one output stream value");
      }
      await cancellableOutput.return();

      return {
        inputOnly,
        outputOnly,
        simultaneous,
        forwarded,
        nestedLabels,
        nestedValues,
        nestedItemLabels,
        nestedItemValues,
        firstSibling,
        secondSibling,
        outputFirst: outputFirstResult.value,
        afterConsumerReturn: await target.ping(),
      };
    },
    async callProducerError() {
      return collect(
        await tsStreamingTargetClient({
          name: this.name,
        }).produceError(),
      );
    },
    async callStreamFree() {
      return tsStreamingTargetClient({
        name: this.name,
      }).incrementScalar();
    },
  },
});

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function collect<T>(stream: AgentStream<T>): Promise<T[]> {
  const values: T[] = [];
  for await (const value of stream) {
    values.push(value);
  }
  return values;
}

function arrayChunks<T>(array: T[], chunkSize: number): T[][] {
  const chunks: T[][] = [];

  for (let i = 0; i < array.length; i += chunkSize) {
    chunks.push(array.slice(i, i + chunkSize));
  }

  return chunks;
}
