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

The SDK uses Standard Schema-compatible schemas to define agent identities,
method inputs, and method results.

From `sdks/ts`, run `pnpm build` to build all TypeScript packages and
`pnpm test` to run their tests.

## Tool middleware

Typed middleware declares installation parameters with `parameterSchema`; the decoded static type is available as `context.parameters`. Omit it for the normalized empty-record `{}` schema. Universal middleware uses the same option. The compile-backed forms are exercised in `tests/tool.test-d.ts` and `tests/tool-middleware.test.ts`.

Typed underlying methods retain the convenient awaited call and add `.start(args)`. A started call exposes an independent `result` promise, optional `stdout`, and `cancel()`, so calls may overlap and their results and streams may be consumed in either order. Universal middleware uses `await underlying.invoke(...)` for the same started shape, or `underlying.invokeAndAwait(...)` for the convenience form.

The underlying is revoked when the middleware handler returns: no new calls may start, but already admitted calls are not implicitly cancelled. Releasing an invocation result observer is disposal, not cancellation; call `cancel()` explicitly when intended. The SDK disposes abandoned observers and streams.

For commands declaring stdout, the guest receives a host stdout writer. The SDK forwards the middleware's selected stdout to it concurrently with the result, finishes it on clean EOF, and fails it on forwarding errors. Authors return or select the readable stdout stream and do not write to or finish the host writer directly.
