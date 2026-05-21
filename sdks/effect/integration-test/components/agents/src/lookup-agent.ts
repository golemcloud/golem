/**
 * Lookup + LookupCaller — exercises the typed-error round-trip across an
 * RPC boundary.
 *
 * `Lookup` declares two methods with non-Void `error:` schemas:
 *
 *   - `fetch({ id })` — `Schema.Number` success / `NotFoundError`
 *     failure. Fails when `id === "missing"`.
 *   - `cmd({ fail })` — `Schema.Void` success / `NotFoundError`
 *     failure. Fails when `fail` is true.
 *
 * The on-the-wire response is a component-model `result<S, E>` carried
 * by the success-DataValue (NOT `AgentError.custom-error`); for the
 * void-success method the success arm uses an empty-record stand-in
 * substituted automatically by the SDK.
 *
 * `LookupCaller` drives `Lookup` over `Lookup.client.get(...)`
 * (intra-component RPC) and reports — as plain `Schema.String` — what
 * the typed-E channel actually delivered. The integration harness then
 * just regex-matches the printed string. Each scenario specifically
 * verifies that the typed user error reaches the caller in the typed
 * `Effect.fail` channel (NOT as the legacy `RpcCallError` wrapper that
 * the original SDK always produced).
 */
import { Effect, Schema } from "effect"
import { defineAgent, method } from "effect-golem"

export const NotFoundError = Schema.Struct({
  _tag: Schema.Literal("NotFoundError"),
  resource: Schema.String,
})

export const Lookup = defineAgent({
  name: "Lookup",
  description: "Demo agent with typed-error methods (Schema.Result wire envelope)",
  mode: "durable",
  constructorParams: { realm: Schema.String },
  methods: {
    /** Returns 7 on success; fails with NotFoundError when id === "missing". */
    fetch: method({
      params: { id: Schema.String },
      success: Schema.Number,
      error: NotFoundError,
    }),
    /** Void on success; fails with NotFoundError when fail is true. */
    cmd: method({
      params: { fail: Schema.Boolean },
      success: Schema.Void,
      error: NotFoundError,
    }),
  },
}).implement(() =>
  Effect.succeed({
    fetch: ({ id }) =>
      id === "missing"
        ? Effect.fail({ _tag: "NotFoundError" as const, resource: id })
        : Effect.succeed(7),
    cmd: ({ fail }) =>
      fail ? Effect.fail({ _tag: "NotFoundError" as const, resource: "always" }) : Effect.void,
  }),
)

/**
 * Discriminate between the typed user error (`NotFoundError`) and any
 * transport-layer surface (`RemoteCallError` etc.). The typed surface
 * is the line the harness regex-matches to confirm the round-trip.
 */
const formatLookupError = (e: unknown): string => {
  const tag = (e as { _tag?: unknown })._tag
  if (tag === "NotFoundError") {
    const resource = (e as { resource: string }).resource
    return `typed:NotFoundError(${resource})`
  }
  return `transport:${JSON.stringify(e)}`
}

export const LookupCaller = defineAgent({
  name: "LookupCaller",
  description: "Drives Lookup over RPC; reports what the typed-E channel delivered",
  mode: "durable",
  constructorParams: { realm: Schema.String },
  methods: {
    /**
     * RPC-call `Lookup.fetch({ id })`. Returns:
     *   - `ok:N` on success
     *   - `typed:NotFoundError(<resource>)` when the typed E channel
     *     delivers (this is the line the harness looks for to confirm
     *     the round-trip works)
     *   - `transport:<json>` for any non-typed RemoteCallError
     */
    fetchAndReport: method({
      params: { id: Schema.String },
      success: Schema.String,
    }),
    /** Same but for the Schema.Void success / typed-error variant. */
    cmdAndReport: method({
      params: { fail: Schema.Boolean },
      success: Schema.String,
    }),
  },
}).implement(({ realm }) =>
  Effect.succeed({
    fetchAndReport: ({ id }) =>
      Effect.gen(function* () {
        const lookup = yield* Lookup.client.get({ realm })
        return yield* lookup.fetch({ id }).pipe(
          Effect.map((n) => `ok:${n}`),
          Effect.catch((e: unknown) => Effect.succeed(formatLookupError(e))),
        )
      }),
    cmdAndReport: ({ fail }) =>
      Effect.gen(function* () {
        const lookup = yield* Lookup.client.get({ realm })
        return yield* lookup.cmd({ fail }).pipe(
          Effect.map(() => "ok:void"),
          Effect.catch((e: unknown) => Effect.succeed(formatLookupError(e))),
        )
      }),
  }),
)
