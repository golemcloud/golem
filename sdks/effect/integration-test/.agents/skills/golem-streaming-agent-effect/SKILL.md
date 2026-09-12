---
name: golem-streaming-agent-effect
description: Adds nested input and output streams to Effect Golem agents. Use for incremental or large RPC values.
---

# Effect agent streams

Declare streams with the SDK schema and bridge them to scoped Effect streams:

```ts
import { Stream, Schema } from "effect"
import { AgentStream, Schema as GolemSchema, method } from "@golemcloud/effect-golem"

const doubled = method({
  input: { values: GolemSchema.AgentStream(Schema.Number) },
  success: GolemSchema.AgentStream(Schema.Number),
})

const handler = ({ values }: { values: AgentStream.AgentStream<number> }) =>
  values.toEffect(String).pipe(
    Stream.map((value) => value * 2),
    AgentStream.AgentStream.fromEffect,
  )
```

`toEffect` requires an error-mapping function. `AgentStream.fromEffect` returns an
`Effect<AgentStream<...>>`, so return that Effect from the handler; do not unwrap it or return the
intermediate Effect `Stream`.

Keep consumption and RPC clients scoped. Streams may be nested in structs. Never convert a potentially large stream to an array just to cross RPC; interruption must release the producer.
