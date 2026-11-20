import { Schema } from "effect"
import { InvocationContext, Tool } from "../src/index.js"

declare const live: Schema.Codec<InvocationContext.Principal>
declare const liveCaller: Schema.Codec<InvocationContext.Caller>
declare const liveToken: Schema.Codec<InvocationContext.TokenProvenance>

// A Principal, a Caller and a TokenProvenance are live values: a tool refuses them in an option,
// a positional, a tail, a global option, a result and an error payload, as a method does.
Tool.toolDefinition("option-tool").body((b) =>
  // @ts-expect-error a Caller cannot be a tool option
  b.option("who", liveCaller),
)
Tool.toolDefinition("positional-tool").body((b) =>
  // @ts-expect-error a TokenProvenance cannot be a tool positional
  b.positional("token", liveToken),
)
Tool.toolDefinition("tail-tool").body((b) =>
  // @ts-expect-error a Principal cannot be a tool tail
  b.tail("who", live),
)
Tool.toolDefinition("global-tool").body((b) =>
  // @ts-expect-error a Principal cannot be a global option
  b.globalOption("who", live),
)
Tool.toolDefinition("result-tool").body((b) =>
  // @ts-expect-error a Principal cannot be a tool result
  b.returns(live),
)
Tool.toolDefinition("nested-tool").body((b) =>
  // @ts-expect-error a Principal cannot be nested in a tool result
  b.returns(Schema.Struct({ who: live })),
)
Tool.toolDefinition("error-tool").body((b) =>
  // @ts-expect-error a Principal cannot be a tool error payload
  b.error("bad", live),
)

// Plain data, a PrincipalRef included, is accepted.
Tool.toolDefinition("ok-tool").body((b) =>
  b
    .option("name", Schema.String)
    .positional("owner", InvocationContext.PrincipalRefSchema)
    .returns(Schema.Struct({ ok: Schema.Boolean })),
)
