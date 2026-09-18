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

## Runtime tool reflection

Use `reflection.getToolType(name)` or `reflection.getAllToolTypes()` to discover tools visible to
the calling component. A `ToolType` is an immutable metadata snapshot. Resolve command names or
aliases with `tool.client.command(path)`; the returned command exposes its canonical `path`,
ordered arguments, input schema, result schema, and child commands. Namespace commands appear in
the tree but cannot be invoked until a command body is selected.

```ts
import { reflection } from '@golemcloud/golem-ts-sdk';

const tool = reflection.getToolType('weather');
if (tool) {
  const forecast = tool.client.command(['forecast']);
  const result = await forecast.invokeJson({ city: 'Budapest' });
}
```

Canonical JSON records include every argument key; use `null` for an absent optional value.
`invokeValue` accepts a schema-native value. Both forms validate against the discovered schema
before opening RPC and check declared results after invocation. `startJson` and `startValue`
expose stdout, result, `collect`, and cancellation for pending calls; use them when stdout is
required. `DynamicToolClient` accepts a caller-packed typed schema value without pretending to
know the deployed schema. Reflected output failures reject with `ToolRemoteOutputError`.

The SDK uses Standard Schema-compatible schemas to define agent identities,
method inputs, and method results.

From `sdks/ts`, run `pnpm build` to build all TypeScript packages and
`pnpm test` to run their tests.
