import { Effect, Schema } from "effect"
import { Tool } from "@golemcloud/effect-golem"

Tool.toolDefinition("double", { version: "1.0.0", requiresFilesystem: true })
  .body((body) => body.positional("value", Schema.Number).returns(Schema.Number))
  .implement({ double: ({ value }) => Effect.succeed(value * 2) })
