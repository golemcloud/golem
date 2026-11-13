import { Cause, Effect, Exit, Fiber, Result, Schema } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type * as DurabilityHost from "golem:durability/durability@1.6.0"
import {
  CustomInvocationContextClient,
  type CustomScopeToken,
} from "../host/CustomInvocationContextClient.js"
import { DurabilityClient } from "../host/DurabilityClient.js"
import { toWitCodec, UnsupportedSchemaError, type WitCodec } from "../WitCodec.js"
import type { SchemaValue } from "./schema-model/model.js"
import { typedSchemaValueFromWit, typedSchemaValueToWit } from "./schema-model/wit.js"
import { DurabilityHostError } from "./durabilityMode.js"
import {
  awaitChildren,
  closeScope,
  CurrentCustomScope,
  currentCustomScope,
  registerChild,
  releaseChild,
} from "./customScope.js"

const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/** @since 1.6.0 @category models */
export type DurableFunctionType = DurabilityHost.DurableFunctionType
/** @since 1.6.0 @category models */
export type PersistedDurableFunctionInvocation = DurabilityHost.PersistedDurableFunctionInvocation

/** @since 1.6.0 @category errors */
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
    this.message = `DurabilityReplayMismatchError: expected ${expectedFunctionName}/${expectedFunctionType.tag}, got ${actualFunctionName}/${actualFunctionType.tag}`
  }
}

/** @since 1.6.0 @category errors */
export class DurabilityDecodeError {
  readonly _tag = "DurabilityDecodeError"
  readonly [sdkErrorBrand] = true
  readonly message: string
  constructor(
    readonly phase: "request-encode" | "response-encode" | "response-decode",
    readonly cause: unknown,
  ) {
    this.message = `DurabilityDecodeError(${phase}): ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/** @since 1.6.0 @category constructors */
export const FunctionType = {
  readLocal: { tag: "read-local" } as const,
  writeLocal: { tag: "write-local" } as const,
  readRemote: { tag: "read-remote" } as const,
  writeRemote: { tag: "write-remote" } as const,
} as const

void ({
  "read-local": FunctionType.readLocal,
  "write-local": FunctionType.writeLocal,
  "read-remote": FunctionType.readRemote,
  "write-remote": FunctionType.writeRemote,
} satisfies Record<DurableFunctionType["tag"], unknown>)

/** @since 1.6.0 @category host bindings */
export const observeFunctionCall = (
  iface: string,
  function_: string,
): Effect.Effect<void, DurabilityHostError, DurabilityClient> =>
  Effect.flatMap(DurabilityClient, (client) =>
    Effect.try({
      try: () => client.observeFunctionCall(iface, function_),
      catch: (cause) => new DurabilityHostError(cause),
    }),
  )

/** @since 1.6.0 @category models */
export interface DurabilityWrapOptions<
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top,
> {
  readonly iface: string
  readonly function: string
  readonly functionType: DurableFunctionType
  readonly requestSchema: RequestS
  readonly success: SuccessS
  readonly error?: ErrorS
  readonly forcedCommit?: boolean
}

/** @since 1.6.0 @category models */
export interface DurabilityWrapInfallibleOptions<
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
> {
  readonly iface: string
  readonly function: string
  readonly functionType: DurableFunctionType
  readonly requestSchema: RequestS
  readonly success: SuccessS
  readonly forcedCommit?: boolean
}

const encodeTyped = <S extends Schema.Top>(
  codec: WitCodec<S>,
  value: S["Type"],
  phase: "request-encode" | "response-encode",
) =>
  (
    Schema.encodeEffect(codec.codec)(value) as Effect.Effect<
      SchemaValue,
      unknown,
      S["EncodingServices"]
    >
  ).pipe(
    Effect.map((encoded) => typedSchemaValueToWit({ graph: codec.graph, value: encoded })),
    Effect.mapError((cause) => new DurabilityDecodeError(phase, cause)),
  )

const decodeTyped = <S extends Schema.Top>(codec: WitCodec<S>, value: CoreTypes.TypedSchemaValue) =>
  Effect.try({
    try: () => typedSchemaValueFromWit(value),
    catch: (cause) => new DurabilityDecodeError("response-decode", cause),
  }).pipe(
    Effect.flatMap((typed) => Schema.decodeEffect(codec.codec)(typed.value)),
    Effect.mapError((cause) =>
      cause instanceof DurabilityDecodeError
        ? cause
        : new DurabilityDecodeError("response-decode", cause),
    ),
  )

type SdkInternalError =
  | DurabilityHostError
  | DurabilityReplayMismatchError
  | DurabilityDecodeError
  | UnsupportedSchemaError
const isSdkInternalError = (error: unknown): error is SdkInternalError =>
  error !== null &&
  typeof error === "object" &&
  (error as { [key: symbol]: unknown })[sdkErrorBrand] === true
const sdkErrorsToDefects = <A, E, R>(effect: Effect.Effect<A, E, R>) =>
  effect.pipe(Effect.catchIf(isSdkInternalError, (error) => Effect.die(error))) as Effect.Effect<
    A,
    Exclude<E, SdkInternalError>,
    R
  >

const sameFunctionType = (left: DurableFunctionType, right: DurableFunctionType): boolean =>
  left.tag === right.tag

const tryHostPromise = <A>(f: () => Promise<A>) =>
  Effect.tryPromise({ try: f, catch: (cause) => new DurabilityHostError(cause) })

/**
 * Runs a live body in its own child fiber, in the operation's scope, and finishes the
 * operation once the body and every child operation registered under its scope ended.
 * The live invocation is always dropped under its own scope, then the scope is closed.
 * Runs uninterruptibly; `restore` makes only the wait for the body interruptible.
 */
const runLive = <A, E, R, RF>(
  restore: <AX, EX, RX>(effect: Effect.Effect<AX, EX, RX>) => Effect.Effect<AX, EX, RX>,
  parent: CustomScopeToken,
  live: DurabilityHost.LiveCustomDurableInvocation,
  body: Effect.Effect<A, E, R>,
  finishWith: (
    exit: Exit.Exit<A, E>,
  ) => Effect.Effect<CoreTypes.TypedSchemaValue | undefined, DurabilityDecodeError, RF>,
  forcedCommit: boolean,
) =>
  Effect.gen(function* () {
    const client = yield* DurabilityClient
    const scopes = yield* CustomInvocationContextClient
    let token: CustomScopeToken
    try {
      token = scopes.run(parent, () => scopes.enterLive(live))
    } catch (cause) {
      client.drop(live)
      throw cause
    }
    const cleanup = Effect.sync(() => {
      try {
        scopes.run(token, () => client.drop(live))
      } finally {
        closeScope(scopes, token, parent)
      }
    })
    return yield* Effect.gen(function* () {
      const fiber = yield* Effect.forkChild(Effect.scoped(body)).pipe(
        Effect.provideService(CurrentCustomScope, token),
      )
      const awaited = yield* Effect.exit(restore(Fiber.await(fiber)))
      if (Exit.isFailure(awaited)) {
        yield* Fiber.interrupt(fiber)
        yield* awaitChildren(token)
        return yield* Effect.failCause(awaited.cause)
      }
      const exit = awaited.value
      yield* awaitChildren(token)
      const response = yield* finishWith(exit)
      if (response !== undefined) {
        yield* tryHostPromise(() => client.finishAsync(live, response, forcedCommit)).pipe(
          Effect.provideService(CurrentCustomScope, token),
        )
      }
      return yield* exit
    }).pipe(Effect.ensuring(cleanup))
  })

const run = <
  RequestS extends Schema.Top,
  SuccessS extends Schema.Top,
  ErrorS extends Schema.Top,
  R,
>(
  opts: DurabilityWrapOptions<RequestS, SuccessS, ErrorS>,
  request: RequestS["Type"],
  body: Effect.Effect<SuccessS["Type"], ErrorS["Type"], R>,
  error: ErrorS | undefined,
) =>
  Effect.gen(function* () {
    const client = yield* DurabilityClient
    const requestCodec = yield* toWitCodec(opts.requestSchema)
    const responseCodec = (yield* toWitCodec(
      error === undefined ? opts.success : Schema.Result(opts.success, error),
    )) as WitCodec<Schema.Top>
    const requestValue = yield* encodeTyped(requestCodec, request, "request-encode")
    const parent = yield* currentCustomScope
    yield* observeFunctionCall(opts.iface, opts.function)
    const functionName = opts.iface === "" ? opts.function : `${opts.iface}::${opts.function}`
    const replay = (entry: DurabilityHost.PersistedDurableFunctionInvocation) =>
      Effect.gen(function* () {
        if (
          entry.functionName !== functionName ||
          !sameFunctionType(entry.functionType, opts.functionType)
        ) {
          return yield* Effect.fail(
            new DurabilityReplayMismatchError(
              functionName,
              entry.functionName,
              opts.functionType,
              entry.functionType,
              entry.entryVersion,
            ),
          )
        }
        const decoded = yield* decodeTyped(responseCodec, entry.response)
        if (error === undefined) return decoded as SuccessS["Type"]
        const result = decoded as Result.Result<SuccessS["Type"], ErrorS["Type"]>
        return Result.isSuccess(result) ? result.success : yield* Effect.fail(result.failure)
      })
    // Defects, interruptions and response encode failures leave the invocation unfinished.
    const finishWith = (exit: Exit.Exit<SuccessS["Type"], ErrorS["Type"]>) => {
      if (Exit.isSuccess(exit))
        return encodeTyped(
          responseCodec,
          error === undefined ? exit.value : Result.succeed(exit.value),
          "response-encode",
        )
      if (Cause.hasDies(exit.cause) || Cause.hasInterrupts(exit.cause)) return Effect.undefined
      const failure = exit.cause.reasons.find(Cause.isFailReason)
      return failure !== undefined && error !== undefined
        ? encodeTyped(responseCodec, Result.fail(failure.error), "response-encode")
        : Effect.undefined
    }
    // The operation counts as a child of its parent from before admission, so the parent cannot
    // finish while the host starts it. A live admission must be entered and dropped, so admission
    // and the live run are uninterruptible apart from the wait for the body.
    const done = yield* registerChild(parent)
    return yield* Effect.uninterruptibleMask((restore) =>
      Effect.gen(function* () {
        const invocation = yield* tryHostPromise(() =>
          client.beginCustomDurableInvocationAsync(functionName, requestValue, opts.functionType),
        )
        return invocation.tag === "replayed"
          ? yield* restore(replay(invocation.val))
          : yield* runLive(
              restore,
              parent,
              invocation.val,
              body,
              finishWith,
              opts.forcedCommit ?? false,
            )
      }),
    ).pipe(Effect.ensuring(releaseChild(parent, done)))
  })

/**
 * Records a custom operation's result. Sequential and properly nested execution only.
 * The body may await host calls when no unrelated guest work runs during the wait. Do not overlap
 * it with other fibers, concurrent effects, or JavaScript promises, including ordinary host calls:
 * sibling effects can acquire the wrong scope and cause incorrect replay or invocation failure.
 * Misuse is not reliably detected. Await nested operations; do not detach work. Stop and join
 * outstanding work before the body exits, including on failure or interruption. Structured
 * finalization alone does not make concurrent custom scopes supported.
 * @since 1.6.0
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
> => sdkErrorsToDefects(run(opts, request, body, opts.error)) as never

/**
 * Infallible form of {@link wrap}, with the same sequential-only and no-detached-work limits.
 * An infallible body can still be interrupted; all children must terminate before its scope closes.
 * @since 1.6.0
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
> => sdkErrorsToDefects(run({ ...opts, error: undefined }, request, body, undefined)) as never
