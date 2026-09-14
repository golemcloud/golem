/** Runtime support for exact graph-backed generated tool bridges. @since 1.6.0 */
import type * as Host from "golem:tool/host@0.1.0"
import type * as Common from "golem:tool/common@0.1.0"
import { Effect, Option, Scope, Stream } from "effect"
import * as Bridge from "./Bridge.js"
import { ToolClient } from "./host/ToolClient.js"
import { liveToolStart, ToolTransport } from "./Tool.js"
import { deepEqual, type TypedSchemaValue } from "./internal/schema-model/model.js"
import { schemaValueMatches } from "./internal/reflection/schemaValidation.js"
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueToWitAsync,
} from "./internal/schema-model/wit.js"

export * from "./Bridge.js"
export { ToolTransport }
export type { ToolTransport as ToolTransportShape } from "./Tool.js"

/** Effect input stream accepted by generated clients. @since 1.6.0 @category streams */
export type ToolInputStream<E = never, R = never> = Stream.Stream<Uint8Array, E, R>
/** Low-level result returned by a tool invocation. @since 1.6.0 @category models */
export interface ToolInvocationResult {
  readonly result?: Common.TypedSchemaValue
}
/** Runtime error preserved by generated clients. @since 1.6.0 @category errors */
export type ToolRuntimeError<E> =
  | { readonly tag: "rpc"; readonly error: Host.RpcError }
  | { readonly tag: "tool"; readonly error: E }
/** Host and lifetime requirements of a generated invocation. @since 1.6.0 @category models */
export type ToolRequirements = ToolClient | Scope.Scope

/** Started streaming invocation exposed by generated clients. @since 1.6.0 @category streams */
export interface StartedToolInvocation<A, E, R = never> {
  readonly stdout: Stream.Stream<Uint8Array, ToolRuntimeError<E>>
  readonly result: Effect.Effect<A, ToolRuntimeError<E>, R>
  readonly cancel: Effect.Effect<void>
}

/** Runtime seam consumed by generated clients. @since 1.6.0 @category models */
export interface ToolClientRuntime {
  start<E>(
    path: readonly string[],
    input: TypedSchemaValue,
    stdin: ToolInputStream<unknown, any> | undefined,
    stdout: boolean,
  ): Effect.Effect<
    {
      readonly stdout?: Stream.Stream<Uint8Array, ToolRuntimeError<E>>
      readonly result: Effect.Effect<ToolInvocationResult, ToolRuntimeError<E>>
      readonly cancel: Effect.Effect<void>
    },
    ToolRuntimeError<E>,
    ToolClient | Scope.Scope
  >
}

const protocol = (context: string, error: unknown): ToolRuntimeError<never> => ({
  tag: "rpc",
  error: {
    tag: "protocol-error",
    val: `${context}${error instanceof Error ? `: ${error.message}` : ""}`,
  },
})

const byteStream = <E>(source: AsyncIterator<Host.ByteStreamItem>) =>
  Stream.fromAsyncIterable({ [Symbol.asyncIterator]: () => source }, (error) =>
    protocol("tool stdout failed", error),
  ).pipe(
    Stream.mapEffect((item) =>
      item.tag === "ok"
        ? Effect.succeed(item.val)
        : Effect.fail(protocol("tool stdout failed", item.val)),
    ),
  ) as Stream.Stream<Uint8Array, ToolRuntimeError<E>>

/** Adapt the contextual Effect tool transport to the exact generated-client runtime. @since 1.6.0 @category constructors */
export const createToolClientRuntime = (tool: string): ToolClientRuntime => ({
  start: <E>(
    path: readonly string[],
    input: TypedSchemaValue,
    stdin: ToolInputStream<unknown, any> | undefined,
    stdout: boolean,
  ) =>
    Effect.gen(function* () {
      const transport = yield* Effect.serviceOption(ToolTransport)
      const inputStream = stdin
        ? yield* Stream.toAsyncIterableEffect(
            Stream.map(stdin, (val): Host.ByteStreamItem => ({ tag: "ok", val })),
          )
        : undefined
      const wireInput = {
        graph: schemaGraphToWit(input.graph),
        value: yield* Effect.tryPromise({
          try: (signal) => schemaValueToWitAsync(input.value, signal),
          catch: (error) => protocol("failed to encode tool input", error),
        }),
      }
      const invocation = yield* Effect.acquireRelease(
        Option.isSome(transport)
          ? transport.value
              .start(tool, path, wireInput, inputStream, stdout)
              .pipe(Effect.mapError((error) => protocol("tool invocation failed", error)))
          : liveToolStart(tool, path, wireInput, inputStream, stdout).pipe(
              Effect.mapError((error) => protocol("tool invocation failed", error)),
            ),
        (started) => started.cancel.pipe(Effect.ignoreCause),
      )
      const stdoutIterator = invocation.stdout?.[Symbol.asyncIterator]()
      if (stdoutIterator?.return) {
        yield* Effect.addFinalizer(() =>
          Effect.tryPromise({
            try: () => stdoutIterator.return!(),
            catch: () => undefined,
          }).pipe(Effect.ignoreCause),
        )
      }
      return {
        stdout: stdoutIterator ? byteStream<E>(stdoutIterator) : undefined,
        result: invocation.result.pipe(
          Effect.map((result) => ({ result: result.result })),
          Effect.mapError((error) =>
            isRpcError(error)
              ? ({ tag: "rpc", error } as ToolRuntimeError<E>)
              : protocol("tool result failed", error),
          ),
        ),
        cancel: invocation.cancel,
      }
    }),
})

/** Create the generated root client. Transport overrides are supplied with `Effect.provideService`. @since 1.6.0 @category constructors */
export const client = <C>(root: { create(runtime: ToolClientRuntime): C }, tool: string): C =>
  root.create(createToolClientRuntime(tool))

/** Construct a streaming generated invocation. @since 1.6.0 @category constructors */
export const startedToolInvocation = <A, E, R>(
  stdout: Stream.Stream<Uint8Array, ToolRuntimeError<E>>,
  result: Effect.Effect<A, ToolRuntimeError<E>, R>,
  cancel: Effect.Effect<void>,
): StartedToolInvocation<A, E, R> => ({ stdout, result, cancel })

/** Check the exact graph shape before generated decoding. @since 1.6.0 @category codecs */
export const typedSchemaValueConforms = (expected: Bridge.SchemaGraph, actual: TypedSchemaValue) =>
  deepEqual(expected, actual.graph) && schemaValueMatches(expected, expected.root, actual.value)

/** Lift and decode a typed wire value under one capability transaction. @since 1.6.0 @category codecs */
export const decodeTypedSchemaValue = <A>(
  wire: Common.TypedSchemaValue,
  decode: (value: TypedSchemaValue) => A,
): A => {
  let graph: Bridge.SchemaGraph
  try {
    graph = schemaGraphFromWit(wire.graph)
  } catch (error) {
    return Bridge.decodeWire(wire.value, () => {
      throw error
    })
  }
  return Bridge.decodeWire(wire.value, (value) => decode({ graph, value: value! }))
}

/** Identify a WIT RPC error. @since 1.6.0 @category errors */
export const isRpcError = (value: unknown): value is Host.RpcError =>
  typeof value === "object" && value !== null && "tag" in value

/** Split declared custom failures from transport failures. @since 1.6.0 @category errors */
export const splitToolRpcError = <E>(
  error: Host.RpcError,
  decode: (value: TypedSchemaValue) => E,
): ToolRuntimeError<E> =>
  error.tag === "remote-tool-error" && error.val.tag === "custom-error"
    ? { tag: "tool", error: decodeTypedSchemaValue(error.val.val, decode) }
    : { tag: "rpc", error }
