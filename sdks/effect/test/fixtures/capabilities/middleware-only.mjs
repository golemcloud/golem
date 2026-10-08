import { Schema } from "effect"
import { universal } from "@golemcloud/effect-golem/middleware"

universal({
  name: "passthrough",
  parameters: Schema.Struct({}),
  handler: (context, underlying) => underlying.invoke(context.commandPath, context.input),
})
