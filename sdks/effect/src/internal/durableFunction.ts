/**
 * Effect-idiomatic wrapper around `golem:durability/durability@1.5.0`,
 * mirroring the `Durability::new + is_live + persist + replay` triplet
 * that every `golem-ai` Rust library uses.
 *
 * The high-level entry point is {@link wrap} (and the infallible
 * variant {@link wrapInfallible}) which encodes a single durable host
 * call as one combinator.
 *
 * **Example**
 *
 * ```ts
 * import { Durability, Schema } from "effect-golem"
 *
 * const result = yield* Durability.wrap(
 *   {
 *     iface: "myapp",
 *     function: "fetchQuote",
 *     functionType: Durability.FunctionType.writeRemote,
 *     requestSchema: Schema.Struct({ symbol: Schema.String }),
 *     success: QuoteSchema,
 *     error: QuoteErrorSchema,
 *   },
 *   { symbol: "AAPL" },
 *   makeRealCall(symbol), // Effect<Quote, QuoteError>
 * )
 * ```
 *
 * Live mode runs `body` (with `persist-nothing` installed so nested
 * host I/O does not double-record), persists `(request, Result<A, E>)`
 * to the oplog, and returns the original value/typed-failure. Replay
 * mode skips `body`, reads the oplog entry, validates the function
 * name AND function type, decodes the recorded `Result` against the
 * declared schemas, and returns the same value/typed-failure that the
 * live execution produced. Defects and interruption propagate without
 * `persist` and without `end-durable-function` — matching Rust's
 * "panic = abnormal termination" semantics.
 *
 * Bit-compatibility with the official Rust SDK: response envelopes are
 * `Schema.Result(success, error)` so they map to the WIT `result<ok,
 * err>` shape, and the function name is qualified as
 * `${iface}::${function}` exactly like
 * `golem-rust::durability::Durability::persist_serializable`.
 *
 * Nesting is allowed: a typical pattern is to use `wrap` to mark a
 * higher-level persisted block and let the body include other custom
 * or host-side durable calls (including a nested `wrap`). In live
 * mode the body is explicitly wrapped with
 * `withPersistenceLevel(persist-nothing, ...)` (see `runLiveBody`),
 * so inner host I/O does not double-record into the outer block's
 * oplog. In replay mode the body is skipped entirely.
 *
 * Variants beyond the unary subset (`write-remote-batched`,
 * `write-remote-transaction`) are NOT accepted by `wrap` because they
 * imply a multi-step lifecycle the unary combinator does not model.
 * Use the lower-level escape hatches ({@link beginDurableFunction} /
 * {@link endDurableFunction} / {@link persistDurableFunctionInvocation}
 * / {@link readPersistedDurableFunctionInvocation}) to compose those
 * flows manually.
 *
 * @since 1.5.0
 */
import { Cause, Effect, Exit, Result, Schema } from "effect"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type * as DurabilityHost from "golem:durability/durability@1.5.0"
import {
  DurabilityHostError,
  PersistenceLevel,
  withPersistenceLevel,
  type PersistenceLevelValue,
} from "./durabilityMode.js"
import { DurabilityClient } from "../host/DurabilityClient.js"
import { DurabilityModeClient } from "../host/DurabilityModeClient.js"
import { toWitCodec, UnsupportedSchemaError, type WitCodec } from "../WitCodec.js"

/**
 * Nominal brand attached to every SDK-internal error class produced by
 * the durable-function wrapper. We use a `Symbol.for(...)` so the
 * brand survives module re-instantiation (e.g., across the bundled
 * vs. runtime copies of the SDK), and we detect SDK errors by
 * `instanceof`-style symbol presence rather than by `_tag` string —
 * that way a user-defined error that happens to share a `_tag`
 * cannot be mis-routed into the defect channel.
 *
 * @internal
 */
const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

/**
 * Re-export of the WIT `durable-function-type` variant.
 *
 * @since 1.5.0
 * @category models
 */
export type DurableFunctionType = DurabilityHost.DurableFunctionType

/**
 * Re-export of the WIT `durable-execution-state` record.
 *
 * @since 1.5.0
 * @category models
 */
export type DurableExecutionState = DurabilityHost.DurableExecutionState

/**
 * Re-export of the WIT `persisted-durable-function-invocation` record.
 *
 * @since 1.5.0
 * @category models
 */
export type PersistedDurableFunctionInvocation = DurabilityHost.PersistedDurableFunctionInvocation

// `OplogIndex` is also exported from `./DurabilityMode.js` (the
// `golem:api/host@1.5.0` re-export); the two are structurally
// identical so the consumer-facing barrel keeps one canonical export.
type OplogIndex = DurabilityHost.OplogIndex

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised on replay when the next oplog entry's `functionName` or
 * `functionType` does not match what the wrapper expected. Indicates
 * the recorded oplog has drifted from the user's code (e.g. a function
 * was renamed between deploys). Mirrors the panic Rust's
 * `validate_oplog_entry` raises, but as a typed Effect failure.
 *
 * @since 1.5.0
 * @category errors
 */
export class DurabilityReplayMismatchError {
  readonly _tag = "DurabilityReplayMismatchError"
  readonly [sdkErrorBrand] = true
  readonly message: string
  constructor(
    readonly expectedFunctionName: string,
    readonly actualFunctionName: string,
    readonly expectedFunctionType: DurableFunctionType,
    readonly actualFunctionType: DurableFunctionType,
    readonly entryVersion: DurabilityHost.OplogEntryVersion,
  ) {
    this.message =
      `DurabilityReplayMismatchError: expected ${expectedFunctionName}/${expectedFunctionType.tag}` +
      `, got ${actualFunctionName}/${actualFunctionType.tag}`
  }
}

/**
 * Raised when encoding/decoding the `ValueAndType` envelope through
 * the user-supplied schemas fails. The `phase` discriminates which
 * step blew up.
 *
 * @since 1.5.0
 * @category errors
 */
export class DurabilityDecodeError {
  readonly _tag = "DurabilityDecodeError"
  readonly [sdkErrorBrand] = true
  readonly message: string
  constructor(
    readonly phase: "request-encode" | "response-encode" | "response-decode",
    readonly cause: unknown,
  ) {
    this.message = `DurabilityDecodeError(${phase}): ${
      cause instanceof Error ? cause.message : String(cause)
    }`
  }
}

// ---------------------------------------------------------------------------
// FunctionType constructors (pure data)
// ---------------------------------------------------------------------------

/**
 * Pure-data constructors for the WIT `wrapped-function-type` /
 * `durable-function-type` variant. Use the unary tags (`readLocal`,
 * `writeLocal`, `readRemote`, `writeRemote`) with {@link wrap}; the
 * batched/transaction variants are only accepted by the lower-level
 * escape hatches.
 *
 * @since 1.5.0
 * @category constructors
 */
export const FunctionType = {
  /** Local, read-only side effect (PRNG, FS read, …). */
  readLocal: { tag: "read-local" } as const,
  /** Local, mutating side effect (FS write, sandbox exec, …). */
  writeLocal: { tag: "write-local" } as const,
  /** Idempotent remote read (poll job, fetch next page, …). */
  readRemote: { tag: "read-remote" } as const,
  /** Non-idempotent remote write (LLM send, DB insert, …). */
  writeRemote: { tag: "write-remote" } as const,
  /**
   * Multi-step remote write whose pieces are recorded across multiple
   * host calls. Pass `undefined` on the first call (triggers a
   * `BeginRemoteWrite` oplog entry); pass that index on subsequent
   * calls. Must be paired with a manual `endDurableFunction`.
   */
  writeRemoteBatched: (val?: OplogIndex) => ({ tag: "write-remote-batched", val }) as const,
  /** Remote-write transaction analogue of {@link writeRemoteBatched}. */
  writeRemoteTransaction: (val?: OplogIndex) => ({ tag: "write-remote-transaction", val }) as const,
} as const

/**
 * Local WIT-drift exhaustiveness witness for {@link FunctionType}: every
 * tag in `golem:api/oplog@1.5.0.wrapped-function-type` (re-exported as
 * `golem:durability/durability@1.5.0.durable-function-type`) must have a
 * corresponding constructor here. If `golem-types/*.d.ts` is regenerated
 * with a new variant, this `satisfies` clause fails to compile and points
 * directly at the wrapper that needs updating.
 */
void ({
  "read-local": FunctionType.readLocal,
  "write-local": FunctionType.writeLocal,
  "read-remote": FunctionType.readRemote,
  "write-remote": FunctionType.writeRemote,
  "write-remote-batched": FunctionType.writeRemoteBatched,
  "write-remote-transaction": FunctionType.writeRemoteTransaction,
} satisfies Record<DurableFunctionType["tag"], unknown>)

/**
 * Subset of {@link DurableFunctionType} accepted by {@link wrap}.
 *
 * @since 1.5.0
 * @category models
 */
export type UnaryDurableFunctionType =
  | { tag: "read-local" }
  | { tag: "write-local" }
  | { tag: "read-remote" }
  | { tag: "write-remote" }

// ---------------------------------------------------------------------------
// Effect-typed escape hatches (consume DurabilityClient via DI)
// ---------------------------------------------------------------------------

/**
 * Emit a host metric/log line for a (iface, function) pair.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const observeFunctionCall = (
  iface: string,
  function_: string,
): Effect.Effect<void, DurabilityHostError, DurabilityClient> =>
  Effect.gen(function* () {
    const svc = yield* DurabilityClient
    return yield* Effect.try({
      try: () => svc.observeFunctionCall(iface, function_),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Open a durable-function bracket and return the host-issued
 * {@link OplogIndex}. Pair with {@link endDurableFunction}.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const beginDurableFunction = (
  functionType: DurableFunctionType,
): Effect.Effect<OplogIndex, DurabilityHostError, DurabilityClient> =>
  Effect.gen(function* () {
    const svc = yield* DurabilityClient
    return yield* Effect.try({
      try: () => svc.beginDurableFunction(functionType),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Close a durable-function bracket previously opened by
 * {@link beginDurableFunction}.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const endDurableFunction = (
  functionType: DurableFunctionType,
  beginIndex: OplogIndex,
  forcedCommit: boolean = false,
): Effect.Effect<void, DurabilityHostError, DurabilityClient> =>
  Effect.gen(function* () {
    const svc = yield* DurabilityClient
    return yield* Effect.try({
      try: () => svc.endDurableFunction(functionType, beginIndex, forcedCommit),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Read the host's current durable-execution state (live vs replay).
 *
 * @since 1.5.0
 * @category host bindings
 */
export const currentDurableExecutionState: Effect.Effect<
  DurableExecutionState,
  DurabilityHostError,
  DurabilityClient
> = Effect.gen(function* () {
  const svc = yield* DurabilityClient
  return yield* Effect.try({
    try: () => svc.currentDurableExecutionState(),
    catch: (e) => new DurabilityHostError(e),
  })
})

/**
 * Convenience: `true` if side effects should run live (executor is in
 * live mode OR persistence level is `persist-nothing`). Mirrors Rust's
 * `Durability::is_live()`.
 *
 * @since 1.5.0
 * @category getters
 */
export const isLive: Effect.Effect<boolean, DurabilityHostError, DurabilityClient> = Effect.map(
  currentDurableExecutionState,
  (s) => s.isLive || s.persistenceLevel.tag === "persist-nothing",
)

/**
 * Persist a typed durable-function invocation entry to the oplog.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const persistDurableFunctionInvocation = (
  functionName: string,
  request: CoreTypes.ValueAndType,
  response: CoreTypes.ValueAndType,
  functionType: DurableFunctionType,
): Effect.Effect<void, DurabilityHostError, DurabilityClient> =>
  Effect.gen(function* () {
    const svc = yield* DurabilityClient
    return yield* Effect.try({
      try: () =>
        svc.persistDurableFunctionInvocation(functionName, request, response, functionType),
      catch: (e) => new DurabilityHostError(e),
    })
  })

/**
 * Read the next persisted durable-function invocation during replay.
 *
 * @since 1.5.0
 * @category host bindings
 */
export const readPersistedDurableFunctionInvocation: Effect.Effect<
  PersistedDurableFunctionInvocation,
  DurabilityHostError,
  DurabilityClient
> = Effect.gen(function* () {
  const svc = yield* DurabilityClient
  return yield* Effect.try({
    try: () => svc.readPersistedDurableFunctionInvocation(),
    catch: (e) => new DurabilityHostError(e),
  })
})

// ---------------------------------------------------------------------------
// wrap / wrapInfallible — the high-level combinator
// ---------------------------------------------------------------------------

/**
 * Configuration for {@link wrap}. The schemas describe the wire shape
 * recorded into the oplog; bit-compatibility with the Rust SDK depends
 * on `requestSchema` matching whatever shape the Rust side emits
 * (typically `Schema.Struct({...})` with the same field names).
 *
 * @since 1.5.0
 * @category models
 */
export interface DurabilityWrapOptions<
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top,
> {
  /** Interface name (oplog namespace), e.g. `"myapp"`. */
  readonly iface: string
  /** Function name within `iface`, e.g. `"fetchQuote"`. */
  readonly function: string
  /** WIT `durable-function-type` (unary subset). */
  readonly functionType: UnaryDurableFunctionType
  /** Schema for the `request` value; encoded once per live invocation. */
  readonly requestSchema: RequestS
  /** Schema for the success branch of the response. */
  readonly success: SuccessS
  /** Schema for the typed failure branch of the response. */
  readonly error?: ErrorS
  /** Force-commit the oplog after `end-durable-function`. Default `false`. */
  readonly forcedCommit?: boolean
}

/**
 * Configuration for {@link wrapInfallible} — no `error` schema.
 *
 * @since 1.5.0
 * @category models
 */
export interface DurabilityWrapInfallibleOptions<
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
> {
  readonly iface: string
  readonly function: string
  readonly functionType: UnaryDurableFunctionType
  readonly requestSchema: RequestS
  readonly success: SuccessS
  readonly forcedCommit?: boolean
}

// ---- helpers ----------------------------------------------------------

const qualifiedName = (iface: string, fn: string): string => `${iface}::${fn}`

const valueAndTypeOf = <S extends Schema.Top>(
  wc: WitCodec<S>,
  value: S["Type"],
  phase: "request-encode" | "response-encode",
): Effect.Effect<CoreTypes.ValueAndType, DurabilityDecodeError, S["EncodingServices"]> =>
  (
    Schema.encodeEffect(wc.codec)(value) as Effect.Effect<
      CoreTypes.WitValue,
      unknown,
      S["EncodingServices"]
    >
  ).pipe(
    Effect.mapBoth({
      onFailure: (cause) => new DurabilityDecodeError(phase, cause),
      onSuccess: (wv) => ({ value: wv, typ: wc.witType }) as CoreTypes.ValueAndType,
    }),
  )

const decodeWitValue = <S extends Schema.Top>(
  wc: WitCodec<S>,
  vt: CoreTypes.ValueAndType,
): Effect.Effect<S["Type"], DurabilityDecodeError, S["DecodingServices"]> =>
  (
    Schema.decodeEffect(wc.codec)(vt.value) as Effect.Effect<
      S["Type"],
      unknown,
      S["DecodingServices"]
    >
  ).pipe(Effect.mapError((cause) => new DurabilityDecodeError("response-decode", cause)))

const compileResponseSchema = (success: Schema.Top, error: Schema.Top | undefined): Schema.Top => {
  if (error === undefined) return success
  return Schema.Result(success, error) as unknown as Schema.Top
}

/** Same as the bottom branch of `withPersistenceLevel(persistNothing, …)`,
 *  but a no-op when persistence is already suppressed. */
const runLiveBody = <A, E, R>(
  body: Effect.Effect<A, E, R>,
  current: PersistenceLevelValue,
): Effect.Effect<A, E | DurabilityHostError, R | DurabilityModeClient> =>
  current.tag === "persist-nothing"
    ? body
    : withPersistenceLevel(PersistenceLevel.persistNothing, body)

// ---- the combinator ---------------------------------------------------

/**
 * Run `body` as a single durable function invocation. See the module
 * doc-comment for the full live/replay protocol and bit-compatibility
 * guarantees.
 *
 * @since 1.5.0
 * @category combinators
 */
export const wrap = <
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top = Schema.Never,
  R = never,
>(
  opts: DurabilityWrapOptions<RequestS, SuccessS, ErrorS>,
  request: RequestS["Type"],
  body: Effect.Effect<SuccessS["Type"], ErrorS["Type"], R>,
): Effect.Effect<
  SuccessS["Type"],
  ErrorS["Type"],
  | R
  | RequestS["EncodingServices"]
  | SuccessS["EncodingServices"]
  | SuccessS["DecodingServices"]
  | ErrorS["EncodingServices"]
  | ErrorS["DecodingServices"]
> =>
  // Internally widens R with `DurabilityClient` (and anything
  // `withPersistenceLevel` adds, e.g. `DurabilityModeClient`); the
  // cast hides them from user-facing types because the agent
  // dispatcher's `provideUserRuntime` provides them before any user
  // code runs.
  sdkErrorsToDefects(wrapInternal(opts, request, body, opts.error)) as Effect.Effect<
    SuccessS["Type"],
    ErrorS["Type"],
    | R
    | RequestS["EncodingServices"]
    | SuccessS["EncodingServices"]
    | SuccessS["DecodingServices"]
    | ErrorS["EncodingServices"]
    | ErrorS["DecodingServices"]
  >

/**
 * Same as {@link wrap}, but for `Effect<A, never, R>` bodies. The
 * response envelope is a bare `success` value rather than a
 * `Result<A, E>`. Used by stream "begin" markers and other
 * never-failing durable points.
 *
 * @since 1.5.0
 * @category combinators
 */
export const wrapInfallible = <RequestS extends Schema.Top, SuccessS extends Schema.Top, R>(
  opts: DurabilityWrapInfallibleOptions<RequestS, SuccessS>,
  request: RequestS["Type"],
  body: Effect.Effect<SuccessS["Type"], never, R>,
): Effect.Effect<
  SuccessS["Type"],
  never,
  R | RequestS["EncodingServices"] | SuccessS["EncodingServices"] | SuccessS["DecodingServices"]
> =>
  // Same internal-vs-public R discrepancy as {@link wrap}; cast hides
  // `DurabilityClient | DurabilityModeClient` from the user-facing
  // type.
  sdkErrorsToDefects(
    wrapInternal<RequestS, SuccessS, Schema.Never, R>(opts, request, body, undefined),
  ) as Effect.Effect<
    SuccessS["Type"],
    never,
    R | RequestS["EncodingServices"] | SuccessS["EncodingServices"] | SuccessS["DecodingServices"]
  >

/**
 * Reroute SDK-internal failures (host errors, schema/codec failures,
 * replay drift) into the defect channel. The user's typed `E` keeps
 * its original meaning — methods can declare a narrow `error` schema
 * without having to widen it to mention every infrastructure error.
 * Defects propagate through Effect's panic path, which the dispatcher
 * routes to the host's normal failure semantics.
 *
 * Users who want to handle these errors as values can use the
 * lower-level escape hatches ({@link beginDurableFunction} /
 * {@link persistDurableFunctionInvocation} / etc.) directly.
 */
type SdkInternalError =
  | DurabilityHostError
  | DurabilityReplayMismatchError
  | DurabilityDecodeError
  | UnsupportedSchemaError

/**
 * Detect SDK-internal errors via the nominal `sdkErrorBrand` symbol
 * (NOT by `_tag` string). User-defined errors that happen to share a
 * `_tag` with one of our error classes are correctly left alone.
 */
const isSdkInternalError = (e: unknown): e is SdkInternalError =>
  e !== null && typeof e === "object" && (e as { [k: symbol]: unknown })[sdkErrorBrand] === true

const sdkErrorsToDefects = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, Exclude<E, SdkInternalError>, R> =>
  effect.pipe(Effect.catchIf(isSdkInternalError, (e) => Effect.die(e))) as Effect.Effect<
    A,
    Exclude<E, SdkInternalError>,
    R
  >

const wrapInternal = <
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top,
  R,
>(
  opts: {
    readonly iface: string
    readonly function: string
    readonly functionType: UnaryDurableFunctionType
    readonly requestSchema: RequestS
    readonly success: SuccessS
    readonly forcedCommit?: boolean
  },
  request: RequestS["Type"],
  body: Effect.Effect<SuccessS["Type"], ErrorS["Type"], R>,
  error: ErrorS | undefined,
): Effect.Effect<
  SuccessS["Type"],
  | ErrorS["Type"]
  | DurabilityHostError
  | DurabilityReplayMismatchError
  | DurabilityDecodeError
  | UnsupportedSchemaError,
  | R
  | RequestS["EncodingServices"]
  | SuccessS["EncodingServices"]
  | SuccessS["DecodingServices"]
  | ErrorS["EncodingServices"]
  | ErrorS["DecodingServices"]
> => {
  const fnName = qualifiedName(opts.iface, opts.function)
  const forcedCommit = opts.forcedCommit ?? false

  return Effect.gen(function* () {
    // Build the wit codecs once per call. Schema-derived codecs are
    // cheap (graph build); caching them across invocations would
    // couple us to the user's reuse of schema instances.
    const requestWc = yield* toWitCodec(opts.requestSchema)
    const responseSchema = compileResponseSchema(opts.success, error)
    const responseWc = (yield* toWitCodec(responseSchema)) as WitCodec<Schema.Top>

    // Pre-encode the request BEFORE running the live body. If
    // encoding fails we want to bail out before any user-visible
    // side effect runs, and we get to reuse the same VT in both the
    // success and typed-failure persist branches.
    const reqVT = yield* valueAndTypeOf(
      requestWc as WitCodec<Schema.Top>,
      request,
      "request-encode",
    )

    // Nested wraps are allowed: in live mode `runLiveBody` explicitly
    // wraps the body with `withPersistenceLevel(persist-nothing, ...)`
    // so any inner host I/O — including a nested `wrap` — does not
    // double-record into the outer block's oplog. In replay mode the
    // body is skipped entirely.
    return yield* protocol<RequestS, SuccessS, ErrorS, R>({
      fnName,
      functionType: opts.functionType,
      forcedCommit,
      reqVT,
      responseWc,
      hasError: error !== undefined,
      body,
    })
  })
}

// ---- inner protocol ---------------------------------------------------

interface ProtocolInput<SuccessS extends Schema.Top, ErrorS extends Schema.Top, R> {
  readonly fnName: string
  readonly functionType: UnaryDurableFunctionType
  readonly forcedCommit: boolean
  /** Pre-encoded request `ValueAndType`, computed before the body runs. */
  readonly reqVT: CoreTypes.ValueAndType
  readonly responseWc: WitCodec<Schema.Top>
  readonly hasError: boolean
  readonly body: Effect.Effect<SuccessS["Type"], ErrorS["Type"], R>
}

const protocol = <
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top,
  R,
>(
  input: ProtocolInput<SuccessS, ErrorS, R>,
): Effect.Effect<
  SuccessS["Type"],
  ErrorS["Type"] | DurabilityHostError | DurabilityReplayMismatchError | DurabilityDecodeError,
  | R
  | RequestS["EncodingServices"]
  | SuccessS["EncodingServices"]
  | SuccessS["DecodingServices"]
  | ErrorS["EncodingServices"]
  | ErrorS["DecodingServices"]
> =>
  Effect.gen(function* () {
    yield* observeFunctionCall(input.fnName.split("::")[0]!, input.fnName.split("::")[1]!)
    const beginIndex = yield* beginDurableFunction(input.functionType)
    const state = yield* currentDurableExecutionState
    const live = state.isLive || state.persistenceLevel.tag === "persist-nothing"

    if (live) {
      // Live mode: run body under persist-nothing, classify the exit,
      // persist (request, Result<A,E>), close the bracket. Defects /
      // interruption skip BOTH persist and end (Rust does the same on
      // panic).
      //
      // Important: `runLiveBody` invokes host calls
      // (`withPersistenceLevel`) that can themselves fail with
      // `DurabilityHostError`. Those are SDK-internal — they MUST NOT
      // reach the typed-failure branch (which would try to encode them
      // against the user's error schema). Defect them up front.
      const guardedBody = runLiveBody(input.body, state.persistenceLevel).pipe(
        Effect.catchIf(isSdkInternalError, (e) => Effect.die(e)),
      )
      const exit = yield* Effect.exit(guardedBody) as Effect.Effect<
        Exit.Exit<SuccessS["Type"], ErrorS["Type"]>
      >
      if (Exit.isSuccess(exit)) {
        const respValue: unknown = input.hasError ? Result.succeed(exit.value) : exit.value
        const respVT = yield* valueAndTypeOf(input.responseWc, respValue, "response-encode")
        yield* persistDurableFunctionInvocation(
          input.fnName,
          input.reqVT,
          respVT,
          input.functionType,
        )
        yield* endDurableFunction(input.functionType, beginIndex, input.forcedCommit)
        return exit.value
      }
      // Failure path: only TYPED failures get persisted+ended. Defect
      // / interruption surface as-is, leaving the bracket open (the
      // host treats an unended bracket as the call having "not
      // completed", which is the protocol-correct outcome).
      const failReason = exit.cause.reasons.find(Cause.isFailReason)
      if (failReason !== undefined) {
        if (input.hasError) {
          const respVT = yield* valueAndTypeOf(
            input.responseWc,
            Result.fail(failReason.error as ErrorS["Type"]),
            "response-encode",
          )
          yield* persistDurableFunctionInvocation(
            input.fnName,
            input.reqVT,
            respVT,
            input.functionType,
          )
          yield* endDurableFunction(input.functionType, beginIndex, input.forcedCommit)
        }
        // Re-raise the original typed failure (matching Rust which
        // also returns the original Result).
        return yield* Effect.failCause(exit.cause)
      }
      // Defect or interruption — propagate untouched.
      return yield* Effect.failCause(exit.cause)
    }

    // Replay mode: read entry, validate, decode, close bracket.
    const entry = yield* readPersistedDurableFunctionInvocation
    if (entry.functionName !== input.fnName || entry.functionType.tag !== input.functionType.tag) {
      return yield* Effect.fail(
        new DurabilityReplayMismatchError(
          input.fnName,
          entry.functionName,
          input.functionType,
          entry.functionType,
          entry.entryVersion,
        ),
      )
    }
    const decoded = yield* decodeWitValue(input.responseWc, entry.response)
    yield* endDurableFunction(input.functionType, beginIndex, false)
    if (input.hasError) {
      const r = decoded as Result.Result<SuccessS["Type"], ErrorS["Type"]>
      if (Result.isSuccess(r)) return r.success
      return yield* Effect.fail(r.failure)
    }
    return decoded as SuccessS["Type"]
  })
