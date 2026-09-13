import { Context, Effect, Layer, Schema, Stream } from "effect"
import { client, err, toolDefinition } from "../src/Tool.js"
import { typed } from "../src/Middleware.js"

const definition = toolDefinition("typed").command("child", (command) =>
  command.body((body) =>
    body.option("labels", Schema.String, { repeatable: "repeated" }).returns(Schema.Number),
  ),
)

const typedClient = client(definition, {
  transport: { start: () => Effect.die("not called") },
})

void typedClient.child({ labels: ["a", "b"] })
// @ts-expect-error repeatable scalar options are arrays
void typedClient.child({ labels: "a" })
// @ts-expect-error child command input schema is preserved
void typedClient.child({ labels: [1] })
// @ts-expect-error unknown commands are not projected
void typedClient.missing({})

const presented = toolDefinition("presented").body((body) =>
  body.positional("message", Schema.String).returns(Schema.Number),
)
const expected = toolDefinition("expected").body((body) =>
  body.positional("value", Schema.Number).returns(Schema.String),
)
typed({
  name: "adapter",
  presented,
  expected,
  handler: {
    presented: ({ message }, { underlying }) =>
      underlying({ value: message.length }).pipe(Effect.map((value) => value.length)),
  },
})

class Prefix extends Context.Service<Prefix, { readonly value: number }>()("test/Prefix") {}
const implemented = toolDefinition("ordinary-tool")
  .body((body) =>
    body
      .positional("value", Schema.String)
      .output({ required: true })
      .returns(Schema.Number)
      .error("rejected", Schema.Struct({ reason: Schema.String })),
  )
  .command("child-command", (command) => command.body((body) => body.returns(Schema.String)))

implemented.implement(
  {
    ordinaryTool: ({ value }, { stdout }) =>
      stdout!(
        Stream.fromEffect(
          Effect.gen(function* () {
            const prefix = yield* Prefix
            return new Uint8Array([prefix.value, value.length])
          }),
        ),
      ).pipe(Effect.as(value.length)),
    childCommand: () => Effect.succeed("child"),
  },
  Layer.succeed(Prefix, { value: 1 }),
)

// @ts-expect-error every command in the definition requires an implementation
implemented.implement({ ordinaryTool: () => Effect.succeed(1) })
implemented.implement({
  // @ts-expect-error command input is projected from its body schema
  ordinaryTool: ({ value }: { value: number }) => Effect.succeed(value),
  childCommand: () => Effect.succeed("child"),
})
implemented.implement({
  // @ts-expect-error command success is projected from its return schema
  ordinaryTool: () => Effect.succeed("wrong"),
  childCommand: () => Effect.succeed("child"),
})
implemented.implement({
  // @ts-expect-error only declared failures and their payloads are accepted
  ordinaryTool: () => Effect.fail(err("missing", { reason: "no" })),
  childCommand: () => Effect.succeed("child"),
})
implemented.implement({
  // @ts-expect-error Prefix is required by the implementation and must be provided
  ordinaryTool: () => Effect.map(Prefix, ({ value }) => value),
  childCommand: () => Effect.succeed("child"),
})
typed({
  name: "invalid-adapter",
  presented,
  expected,
  handler: {
    // @ts-expect-error presented input is inferred as a string
    presented: ({ message }: { message: number }) => Effect.succeed(message),
  },
})

const inferred = toolDefinition("inferred").body((body) =>
  body.positional("value", Schema.String).returns(Schema.String),
)
typed({
  name: "inferred-adapter",
  presented: inferred,
  handler: {
    inferred: (_input, { underlying }) => {
      // @ts-expect-error omitted expected definition must retain the presented input type
      void underlying({ value: 1 })
      return underlying({ value: "ok" })
    },
  },
})

const kebab = toolDefinition("root-tool").command("child-command", (command) =>
  command.body((body) => body.returns(Schema.String)),
)
type KebabImplementation = import("../src/Middleware.js").TypedImplementation<typeof kebab>
typed({
  name: "kebab-adapter",
  presented: kebab,
  handler: {
    rootTool: {
      childCommand: (_input, { underlying }) => underlying.childCommand({}),
    },
  } satisfies KebabImplementation,
})
