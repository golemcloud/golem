import { Effect, Redacted, Schema } from "effect"
import {
  defineConfig,
  static as staticField,
  type ConfigError,
  type NonSecretOverride,
} from "../src/Config.js"

const OptionalNestedConfig = defineConfig("OptionalNestedConfig", {
  database: Schema.optional(
    Schema.Struct({
      host: Schema.String,
      credentials: Schema.Struct({
        password: Schema.Redacted(Schema.String),
      }),
    }),
  ),
  token: Schema.optional(Schema.Redacted(Schema.String)),
})

const validOverride: NonSecretOverride<typeof OptionalNestedConfig.__fields> = {
  database: { host: "localhost", credentials: {} },
}
void validOverride

const nestedSecretOverride: NonSecretOverride<typeof OptionalNestedConfig.__fields> = {
  database: {
    host: "localhost",
    credentials: {
      // @ts-expect-error secret leaves are not caller-overridable
      password: "leak",
    },
  },
}
void nestedSecretOverride

const optionalSecretOverride: NonSecretOverride<typeof OptionalNestedConfig.__fields> = {
  // @ts-expect-error optional secret fields are not caller-overridable
  token: "leak",
}
void optionalSecretOverride

Effect.gen(function* () {
  const cfg = yield* OptionalNestedConfig
  const host: Effect.Effect<string | undefined, ConfigError> = cfg.database.host
  const password: Effect.Effect<Redacted.Redacted<string | undefined>, ConfigError> = cfg.database
    .credentials.password.get
  void host
  void password
})

const StaticConfig = defineConfig("StaticConfig", {
  settings: staticField(Schema.Struct({ label: Schema.String, count: Schema.Number })),
  key: Schema.Redacted(Schema.String),
})
const wholeStatic: NonSecretOverride<typeof StaticConfig.__fields> = {
  settings: { label: "whole", count: 7 },
}
void wholeStatic
const partialStatic: NonSecretOverride<typeof StaticConfig.__fields> = {
  // @ts-expect-error a static record is one whole value
  settings: { label: "partial" },
}
void partialStatic
const staticSecret: NonSecretOverride<typeof StaticConfig.__fields> = {
  // @ts-expect-error secret fields are not caller-overridable
  key: "leak",
}
void staticSecret
Effect.gen(function* () {
  const config = yield* StaticConfig
  const settings: Effect.Effect<{ readonly label: string; readonly count: number }, ConfigError> =
    config.settings
  void settings
})
