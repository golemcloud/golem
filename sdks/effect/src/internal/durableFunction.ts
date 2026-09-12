import { Cause, Effect, Exit, Result, Schema, Scope } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type * as DurabilityHost from "golem:durability/durability@1.6.0"
import { DurabilityClient } from "../host/DurabilityClient.js"
import { toWitCodec, UnsupportedSchemaError, type WitCodec } from "../WitCodec.js"
import type { SchemaValue } from "./schema-model/model.js"
import { typedSchemaValueFromWit, typedSchemaValueToWit } from "./schema-model/wit.js"
import { DurabilityHostError } from "./durabilityMode.js"

const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/** @since 1.6.0 @category models */
export type DurableFunctionType = DurabilityHost.DurableFunctionType
/** @since 1.6.0 @category models */
export type PersistedDurableFunctionInvocation = DurabilityHost.PersistedDurableFunctionInvocation
type OplogIndex =
  Extract<DurableFunctionType, { tag: "write-remote-batched" }> extends {
    val: infer I | undefined
  }
    ? I
    : never

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
  writeRemoteBatched: (val?: OplogIndex) => ({ tag: "write-remote-batched", val }) as const,
  writeRemoteTransaction: (val?: OplogIndex) => ({ tag: "write-remote-transaction", val }) as const,
} as const

void ({
  "read-local": FunctionType.readLocal,
  "write-local": FunctionType.writeLocal,
  "read-remote": FunctionType.readRemote,
  "write-remote": FunctionType.writeRemote,
  "write-remote-batched": FunctionType.writeRemoteBatched,
  "write-remote-transaction": FunctionType.writeRemoteTransaction,
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

/**
 * Opens an invocation in an Effect scope. An unfinished live resource is dropped
 * when the scope closes, including after interruption or a defect.
 * @since 1.6.0
 * @category host bindings
 */
export const beginCustomDurableInvocation = (
  functionName: string,
  request: CoreTypes.TypedSchemaValue,
  functionType: DurableFunctionType,
): Effect.Effect<
  DurabilityHost.CustomDurableInvocation,
  DurabilityHostError,
  Scope.Scope | DurabilityClient
> =>
  Effect.gen(function* () {
    const client = yield* DurabilityClient
    return yield* Effect.acquireRelease(
      Effect.try({
        try: () => client.beginCustomDurableInvocation(functionName, request, functionType),
        catch: (cause) => new DurabilityHostError(cause),
      }),
      (invocation) =>
        invocation.tag === "live"
          ? Effect.sync(() => client.drop(invocation.val)).pipe(Effect.ignore)
          : Effect.void,
    )
  })

/** @since 1.6.0 @category host bindings */
export const finishCustomDurableInvocation = (
  invocation: DurabilityHost.LiveCustomDurableInvocation,
  response: CoreTypes.TypedSchemaValue,
  forcedCommit = false,
): Effect.Effect<void, DurabilityHostError, DurabilityClient> =>
  Effect.flatMap(DurabilityClient, (client) =>
    Effect.try({
      try: () => client.finish(invocation, response, forcedCommit),
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
  left.tag === right.tag &&
  ((left.tag !== "write-remote-batched" && left.tag !== "write-remote-transaction") ||
    left.val === (right as typeof left).val)

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
  Effect.scoped(
    Effect.gen(function* () {
      const requestCodec = yield* toWitCodec(opts.requestSchema)
      const responseCodec = (yield* toWitCodec(
        error === undefined ? opts.success : Schema.Result(opts.success, error),
      )) as WitCodec<Schema.Top>
      const requestValue = yield* encodeTyped(requestCodec, request, "request-encode")
      yield* observeFunctionCall(opts.iface, opts.function)
      const functionName = opts.iface === "" ? opts.function : `${opts.iface}::${opts.function}`
      const invocation = yield* beginCustomDurableInvocation(
        functionName,
        requestValue,
        opts.functionType,
      )
      if (invocation.tag === "replayed") {
        const entry = invocation.val
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
      }
      const exit = yield* Effect.exit(body)
      if (Exit.isSuccess(exit)) {
        const response = yield* encodeTyped(
          responseCodec,
          error === undefined ? exit.value : Result.succeed(exit.value),
          "response-encode",
        )
        yield* finishCustomDurableInvocation(invocation.val, response, opts.forcedCommit ?? false)
        return exit.value
      }
      if (Cause.hasDies(exit.cause) || Cause.hasInterrupts(exit.cause)) {
        return yield* Effect.failCause(exit.cause)
      }
      const failure = exit.cause.reasons.find(Cause.isFailReason)
      if (failure !== undefined && error !== undefined) {
        const response = yield* encodeTyped(
          responseCodec,
          Result.fail(failure.error),
          "response-encode",
        )
        yield* finishCustomDurableInvocation(invocation.val, response, opts.forcedCommit ?? false)
      }
      return yield* Effect.failCause(exit.cause)
    }),
  )

/** @since 1.6.0 @category combinators */
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

/** @since 1.6.0 @category combinators */
export const wrapInfallible = <RequestS extends Schema.Top, SuccessS extends Schema.Top, R>(
  opts: DurabilityWrapInfallibleOptions<RequestS, SuccessS>,
  request: RequestS["Type"],
  body: Effect.Effect<SuccessS["Type"], never, R>,
): Effect.Effect<
  SuccessS["Type"],
  never,
  R | RequestS["EncodingServices"] | SuccessS["EncodingServices"] | SuccessS["DecodingServices"]
> => sdkErrorsToDefects(run({ ...opts, error: undefined }, request, body, undefined)) as never
