import { z } from 'zod';
import { AgentStream, defineAgent, method, Result, s } from '@golemcloud/golem-ts-sdk';

export const StreamingAgent = defineAgent({
  name: 'StreamingAgent',
  id: { name: z.string() },
  methods: {
    sum: method({
      input: { input: s.stream(z.number()) },
      returns: z.number(),
    }),
    produce: method({ input: {}, returns: s.stream(z.number()) }),
    transform: method({
      input: { prefix: z.string(), input: s.stream(z.number()) },
      returns: s.stream(z.string()),
    }),
    nested: method({ input: {}, returns: s.stream(s.stream(z.number())) }),
    recoverable: method({
      input: {},
      returns: s.stream(s.result(z.number(), z.string())),
    }),
    status: method({ input: {}, returns: z.string() }),
  },
});

function stream<T>(values: Iterable<T>, onCancel: () => void): AgentStream<T> {
  return AgentStream.from(
    (async function* () {
      let completed = false;
      try {
        for (const value of values) yield value;
        completed = true;
      } finally {
        // A remote consumer that stops early closes this iterator.
        if (!completed) onCancel();
      }
    })(),
  );
}

export const StreamingAgentImpl = StreamingAgent.implement({
  init: () => ({ cancelledProducers: 0 }),
  methods: {
    async sum({ input }) {
      let total = 0;
      for await (const value of input) total += value;
      return total;
    },
    produce() {
      return stream([1, 2, 3], () => this.cancelledProducers++);
    },
    transform({ prefix, input }) {
      const state = this;
      return AgentStream.from(
        (async function* () {
          let completed = false;
          try {
            for await (const value of input) yield `${prefix}:${value}`;
            completed = true;
          } finally {
            // Exiting early also closes `input`, propagating cancellation.
            if (!completed) state.cancelledProducers++;
          }
        })(),
      );
    },
    nested() {
      return stream(
        [
          stream([10, 20], () => this.cancelledProducers++),
          stream([30, 40], () => this.cancelledProducers++),
        ],
        () => this.cancelledProducers++,
      );
    },
    recoverable() {
      return stream<Result<number, string>>(
        [Result.ok(1), Result.err('this item could not be produced'), Result.ok(2)],
        () => this.cancelledProducers++,
      );
    },
    status() {
      return `ready (${this.cancelledProducers} cancelled producers)`;
    },
  },
});
