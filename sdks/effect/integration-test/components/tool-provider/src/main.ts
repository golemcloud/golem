import { Effect, Schema, Stream } from "effect"
import { Tool } from "@golemcloud/effect-golem"

const encoder = new TextEncoder()
const decoder = new TextDecoder()

Tool.toolDefinition("effect-streaming")
  .body((body) =>
    body
      .positional("message", Schema.String)
      .input({ required: true })
      .output({ required: true })
      .returns(Schema.String),
  )
  .implement({
    effectStreaming: ({ message }, context) =>
      Effect.gen(function* () {
        if (!context.stdin || !context.stdout) return yield* Effect.die("required streams missing")
        const output = Stream.concat(
          Stream.succeed(encoder.encode(`tool:${message}:`)),
          Stream.map(context.stdin, (chunk) => encoder.encode(decoder.decode(chunk).toUpperCase())),
        )
        yield* context.stdout(output)
        return `accepted:${message}`
      }),
  })
