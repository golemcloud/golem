import { Effect, Redacted, Schema } from "effect"
import { defineConfig, type ConfigError } from "../src/Config.js"

const OptionalNestedConfig = defineConfig("OptionalNestedConfig", {
  database: Schema.optional(
    Schema.Struct({
      host: Schema.String,
      credentials: Schema.Struct({
        password: Schema.Redacted(Schema.String),
      }),
    }),
  ),
})

Effect.gen(function* () {
  const cfg = yield* OptionalNestedConfig
  const host: Effect.Effect<string | undefined, ConfigError> = cfg.database.host
  const password: Effect.Effect<Redacted.Redacted<string | undefined>, ConfigError> = cfg.database
    .credentials.password.get
  void host
  void password
})
