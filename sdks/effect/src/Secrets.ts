import { Effect, Schema } from "effect"
import type { Secret } from "./Capability.js"
import { SecretsClient } from "./host/SecretsClient.js"
import { assertCapabilityReady } from "./internal/schema-model/capabilityTransaction.js"
import { SECRET_INTERNAL } from "./internal/schema-model/secretInternal.js"
import { peekGuestSecretHandle } from "./internal/schema-model/secretHandle.js"
import { compile, type UnsupportedSchemaError } from "./WitCodec.js"

/** Failure from revealing a secret through the host. @since 1.6.0 @category errors */
export class SecretsHostError {
  readonly _tag = "SecretsHostError"
  readonly message: string

  constructor(readonly cause: unknown) {
    this.message = `SecretsHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/** Reveal an opaque secret using its declared payload schema. @since 1.6.0 @category secrets */
export const reveal = <S extends Schema.Top>(
  secret: Secret,
  schema: S,
): Effect.Effect<
  S["Type"],
  SecretsHostError | Schema.SchemaError | UnsupportedSchemaError,
  SecretsClient
> =>
  Effect.gen(function* () {
    yield* Effect.try({
      try: () => assertCapabilityReady(secret),
      catch: (cause) => new SecretsHostError(cause),
    })
    const raw = peekGuestSecretHandle(SECRET_INTERNAL, secret)
    if (raw === undefined) {
      return yield* Effect.fail(
        new SecretsHostError(new Error("secret has already been transferred")),
      )
    }
    const codec = yield* compile(schema)
    const host = yield* SecretsClient
    const value = yield* Effect.try({
      try: () => host.reveal(raw, codec.schemaGraph),
      catch: (cause) => new SecretsHostError(cause),
    })
    return yield* codec.decode(value)
  }) as Effect.Effect<
    S["Type"],
    SecretsHostError | Schema.SchemaError | UnsupportedSchemaError,
    SecretsClient
  >
