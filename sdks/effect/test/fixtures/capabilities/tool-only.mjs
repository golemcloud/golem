import { Effect, Schema } from "effect"
import { Tool } from "@golemcloud/effect-golem"

Tool.toolDefinition("double", { requiresFilesystem: true })
  .body((body) => body.positional("value", Schema.Number).returns(Schema.Number))
  .implement({ double: ({ value }) => Effect.succeed(value * 2) })
