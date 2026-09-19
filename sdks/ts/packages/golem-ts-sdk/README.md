# Golem TypeScript SDK

```ts
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const counter = defineAgent({
  name: 'Counter',
  id: { name: z.string() },
  methods: {
    increment: method({ input: {}, returns: z.number() }),
  },
});

export const Counter = counter.implement({
  init() {
    return { value: 0 };
  },
  methods: {
    async increment() {
      return ++this.value;
    },
  },
});
```

## Streaming methods

Use `s.stream(itemSchema)` in a method schema and `AgentStream<T>` in its implementation. Streams
can appear at the root or nested inside records, tuples, options, results, variants, and lists.

```ts
import { AgentStream, defineAgent, method, s } from '@golemcloud/golem-ts-sdk';

const numbers = defineAgent({
  name: 'Numbers',
  id: {},
  methods: {
    doubled: method({
      input: { values: s.stream(s.u32()) },
      returns: s.stream(s.u32()),
    }),
  },
});

export const Numbers = numbers.implement({
  init() {
    return {};
  },
  methods: {
    doubled({ values }) {
      return AgentStream.from(
        (async function* () {
          for await (const value of values) yield value * 2;
        })(),
      );
    },
  },
});
```

`AgentStream` is lazy and single-reader, so await each operation before starting another. Clean EOF
is `{ done: true, value: undefined }`. Encoding or forwarding a stream transfers its ownership; do
not reuse the original object.

For a stream received through a connected P3 agent invocation, early exit from `for await` calls
`return()` and closes the readable endpoint. `throw(reason)` also closes it and rejects locally with
`reason`, which is not sent to the producer. When an `AgentStream.from` source is sent through P3,
accepted writes gate later pulls, providing back-pressure. When a subsequent write observes a
remote reader drop, production stops and awaits the source iterator's `return()` exactly once. P3
does not interrupt an arbitrary pending source `next()` or guarantee cleanup before a later agent
invocation. Producer and cleanup failures fail the active operation or invocation session rather
than becoming clean EOF. P3 has no recoverable stream-local terminal error, so model one explicitly
in the item type, for example `stream<result<T, E>>`, when needed.

## Retrying user code

`retry` interprets the same semantic policies used by Golem in ordinary user space. The callback may
return synchronously or return a Promise. Use `properties` when a filtered policy needs values from
the current failure, and use an `AbortSignal` to cancel an attempt or delay.

```ts
import { Duration, Policy, Predicate, retry } from '@golemcloud/golem-ts-sdk';

const policy = Policy.exponential(Duration.milliseconds(100), 2)
  .maxRetries(4)
  .onlyWhen(Predicate.eq('transient', true));

const response = await retry(policy, () => callUnreliableService(), {
  properties: (error) => ({ transient: error instanceof TransientError }),
  signal,
});
```

Aborting stops waiting for an in-flight callback but cannot stop arbitrary user code. Pass the same
signal into the underlying operation when it supports cancellation.

Local `prop-matches` predicates translate `*` to JavaScript regex `.*` and `?` to `.`, without
flags; all other characters are literal. Wildcards exclude line terminators, and `?` matches one
UTF-16 code unit. This does not implement the host's full glob syntax. Use host-driven retries when
authoritative platform matching is required.

These are local function calls, not executor-managed retry attempts. They do not create executor
retry-attempt oplog entries or survive suspension and recovery as one host-managed retry sequence.

The SDK uses Standard Schema-compatible schemas to define agent identities,
method inputs, and method results.

From `sdks/ts`, run `pnpm build` to build all TypeScript packages and
`pnpm test` to run their tests.
