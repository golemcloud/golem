import { Effect, Schema } from "effect"
import { ObjectSchema, readJson } from "./runtime.ts"

const Test = Schema.Struct({
  name: Schema.optional(Schema.Unknown),
  suite: Schema.optional(Schema.Unknown),
  message: Schema.optional(Schema.Unknown),
  flaky: Schema.optional(Schema.Unknown),
  status: Schema.optional(Schema.String),
  extra: Schema.optional(Schema.NullOr(Schema.Struct({
    testR: Schema.optional(Schema.NullOr(Schema.Struct({
      attempts: Schema.optional(Schema.NullOr(Schema.Array(ObjectSchema)))
    })))
  })))
})
const Report = Schema.Struct({
  results: Schema.Struct({
    tests: Schema.Array(Test),
    tool: Schema.optional(Schema.NullOr(ObjectSchema)),
    summary: Schema.optional(Schema.Unknown)
  })
})

/** Validate the entire report before any test contributes totals or coverage. */
export const readReport = Effect.fnUntraced(function*(path: string) {
  return (yield* Schema.decodeUnknownEffect(Report)(yield* readJson(path))).results
})
