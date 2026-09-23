/** Effect-native Golem tool definitions, clients, and guest runtime. @since 1.6.0 */
export * from "./internal/tool/model.js"
export { toolGuest } from "./internal/tool/runtime.js"

import type * as Common from "golem:tool/common@0.1.0"
import type * as Host from "golem:tool/host@0.1.0"
import { Cause, Context, Effect, Exit, Schema, Scope, Stream } from "effect"
import { AbortableStreamIterable } from "./internal/abortableStreamIterable.js"
import { ToolClient } from "./host/ToolClient.js"
import {
  type BodyModel,
  canonicalInputDefaults,
  canonicalInputFields,
  type CommandModel,
  type ToolDefinition,
  registerToolClientFactory,
} from "./internal/tool/model.js"
import { schemaShapesMatch } from "./internal/schema-model/model.js"
import { schemaGraphFromWit } from "./internal/schema-model/wit.js"
import { compile } from "./WitCodec.js"

export { ToolClient } from "./host/ToolClient.js"

/** Convert a kebab-case protocol name to its TypeScript client spelling. @since 1.6.0 @category models */
export type CamelCase<S extends string> = S extends `${infer H}-${infer T}`
  ? `${H}${Capitalize<CamelCase<T>>}`
  : S

/** A failure while preparing, transporting, or decoding a tool invocation. @since 1.6.0 @category errors */
export class ToolClientError extends Error {
  readonly _tag = "ToolClientError"
  constructor(
    readonly phase: "input" | "invoke" | "declared-error" | "result" | "stream",
    readonly cause: unknown,
  ) {
    super(`Tool client ${phase} failed: ${String(cause)}`)
  }
}

/** Invocation returned by an injectable transport. Async iterables stay behind this seam. @since 1.6.0 @category models */
export interface TransportInvocation {
  readonly result: Effect.Effect<Common.InvocationResult, unknown>
  readonly stdout?: AsyncIterable<Host.ByteStreamItem>
  readonly cancel: Effect.Effect<void>
}

/** Injectable low-level transport used by local typed clients. @since 1.6.0 @category models */
export interface ToolTransport {
  readonly start: (
    tool: string,
    path: readonly string[],
    input: Common.TypedSchemaValue,
    stdin: AsyncIterable<Host.ByteStreamItem> | undefined,
    stdout: boolean,
  ) => Effect.Effect<TransportInvocation, unknown>
}

/** Context tag for overriding the generated-client tool transport. @since 1.6.0 @category services */
export const ToolTransport = Context.Service<ToolTransport>("effect-golem/ToolTransport")

/** Application-level input and output streams. @since 1.6.0 @category models */
export interface Streams {
  readonly stdin?: Stream.Stream<Uint8Array, ToolClientError>
  readonly stdout?: (
    stream: Stream.Stream<Uint8Array, ToolClientError>,
  ) => Effect.Effect<void, unknown>
}

/** Typed client options. @since 1.6.0 @category models */
export interface ClientOptions {
  readonly transport?: ToolTransport
  /** Registered leaf name used for host lookup when it differs from metadata identity. */
  readonly lookupName?: string
}

type Input<B extends BodyModel> = {
  readonly [K in keyof B["fields"] as undefined extends B["fields"][K]["Type"]
    ? never
    : CamelCase<string & K>]: B["fields"][K]["Type"]
} & {
  readonly [K in keyof B["fields"] as undefined extends B["fields"][K]["Type"]
    ? CamelCase<string & K>
    : never]?: B["fields"][K]["Type"]
}
type Output<B extends BodyModel> = B["output"] extends Schema.Top ? B["output"]["Type"] : void
type DeclaredError<B extends BodyModel> = B["errors"][number] extends infer E
  ? E extends { readonly name: infer N extends string; readonly schema: infer S extends Schema.Top }
    ? import("./internal/tool/model.js").ToolFailure<N, S["Type"]>
    : never
  : never
type ClientNode<M extends CommandModel, R = never> = (M["body"] extends BodyModel
  ? (
      input: Input<M["body"]>,
      streams?: Streams,
    ) => Effect.Effect<Output<M["body"]>, ToolClientError | DeclaredError<M["body"]>, R>
  : object) & {
  readonly [K in keyof M["children"] as CamelCase<string & K>]: ClientNode<M["children"][K], R>
}
/** Client projected from a tool definition. @since 1.6.0 @category models */
export type Client<D extends ToolDefinition<any, any>, R = never> = ClientNode<D["model"], R>

const streamItems = (source: Stream.Stream<Uint8Array, ToolClientError>) =>
  Stream.map(source, (val): Host.ByteStreamItem => ({ tag: "ok", val }))

const decodeByteStream = (source: AsyncIterable<Host.ByteStreamItem>) =>
  Stream.fromAsyncIterable(source, (cause) => new ToolClientError("stream", cause)).pipe(
    Stream.mapEffect((item) =>
      item.tag === "ok"
        ? Effect.succeed(item.val)
        : Effect.fail(new ToolClientError("stream", item.val)),
    ),
  )

const drainByteStream = (source: AsyncIterable<Host.ByteStreamItem>) =>
  Stream.runDrain(decodeByteStream(source))

/** Start a scoped invocation using the contextual tool host. @since 1.6.0 @category constructors */
export const liveToolStart = (
  tool: string,
  path: readonly string[],
  input: Common.TypedSchemaValue,
  stdin: AsyncIterable<Host.ByteStreamItem> | undefined,
  withStdout: boolean,
  reflected = false,
) =>
  Effect.gen(function* () {
    const host = yield* ToolClient
    const rpc = yield* Effect.try({
      try: () => (reflected ? host.createRpc(tool) : host.rpc(tool)),
      catch: (cause) => new ToolClientError("invoke", cause),
    })
    const inputEndpoints = stdin ? host.createStdin() : undefined
    const output = withStdout ? host.createStdout() : undefined
    const future = yield* Effect.try({
      try: () => rpc.asyncInvokeAndAwait([...path], input, inputEndpoints?.[1], output?.[0]),
      catch: (cause) => new ToolClientError("invoke", cause),
    })
    if (inputEndpoints && stdin) {
      const [writer, , closed] = inputEndpoints
      const pump = decodeByteStream(stdin).pipe(
        Stream.filter((chunk) => chunk.length > 0),
        Stream.runForEach((chunk) =>
          Effect.tryPromise({ try: () => writer.write(chunk), catch: (cause) => cause }),
        ),
        Effect.andThen(Effect.tryPromise({ try: () => writer.finish(), catch: (cause) => cause })),
        Effect.catch((cause) =>
          Effect.promise(() => writer.fail({ tag: "failed", val: String(cause) })).pipe(
            Effect.ignoreCause,
          ),
        ),
      )
      yield* Effect.forkScoped(
        Effect.raceFirst(
          pump,
          Effect.promise(() => closed.wait()),
        ).pipe(Effect.ignoreCause),
      )
    }
    return {
      result: Effect.tryPromise({ try: () => future.get(), catch: (cause) => cause }),
      stdout: output?.[1],
      cancel: Effect.sync(() => future.cancel()),
    }
  })

/** Construct an Effect client from a local definition. @since 1.6.0 @category constructors */
export function client<D extends ToolDefinition<any, any>>(
  definition: D,
  options: ClientOptions & { readonly transport: ToolTransport },
): Client<D>
export function client<D extends ToolDefinition<any, any>>(
  definition: D,
  options?: ClientOptions,
): Client<D, ToolClient>
export function client<D extends ToolDefinition<any, any>>(
  definition: D,
  options: ClientOptions = {},
): Client<D, ToolClient> {
  const start: (
    tool: string,
    path: readonly string[],
    input: Common.TypedSchemaValue,
    stdin: AsyncIterable<Host.ByteStreamItem> | undefined,
    stdout: boolean,
  ) => Effect.Effect<TransportInvocation, unknown, ToolClient | Scope.Scope> =
    options.transport?.start ?? liveToolStart
  const call = (
    model: CommandModel,
    path: readonly string[],
    input: Record<string, unknown>,
    streams?: Streams,
  ) =>
    Effect.scoped(
      Effect.gen(function* () {
        if (!model.body) return yield* Effect.fail(new ToolClientError("input", "missing body"))
        const fields = canonicalInputFields(definition, path)
        const defaults = canonicalInputDefaults(definition, path)
        if (!fields) return yield* Effect.fail(new ToolClientError("input", "missing command"))
        const codec = yield* compile(Schema.Struct(fields)).pipe(
          Effect.mapError((cause) => new ToolClientError("input", cause)),
        )
        const canonicalInput = {
          ...defaults,
          ...Object.fromEntries(
            Object.keys(fields).flatMap((name) => {
              const inputName = name.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())
              return Object.hasOwn(input, inputName) ? [[name, input[inputName]]] : []
            }),
          ),
        }
        const value = yield* codec
          .encodeAsync(canonicalInput)
          .pipe(Effect.mapError((cause) => new ToolClientError("input", cause)))
        const stdin = streams?.stdin
          ? new AbortableStreamIterable(streamItems(streams.stdin), yield* Effect.context<never>())
          : undefined
        if (stdin) yield* Effect.addFinalizer(() => Effect.promise(() => stdin.close()))
        const started = yield* start(
          options.lookupName ?? definition.name,
          path,
          { graph: codec.schemaGraph, value },
          stdin,
          !!model.body.stdout,
        ).pipe(Effect.mapError((cause) => new ToolClientError("invoke", cause)))
        yield* Effect.addFinalizer(() => started.cancel)
        const stdoutIterator = started.stdout?.[Symbol.asyncIterator]()
        const stdout = stdoutIterator ? { [Symbol.asyncIterator]: () => stdoutIterator } : undefined
        const consume = stdout
          ? streams?.stdout
            ? streams
                .stdout(decodeByteStream(stdout))
                .pipe(
                  Effect.catchCause((cause) =>
                    cause.reasons.some(Cause.isInterruptReason)
                      ? Effect.failCause(cause)
                      : drainByteStream(stdout).pipe(
                          Effect.ignore,
                          Effect.andThen(Effect.failCause(cause)),
                        ),
                  ),
                )
            : drainByteStream(stdout)
          : Effect.void
        const invocationResult = started.result.pipe(
          Effect.catchIf(
            (): boolean => true,
            (cause: unknown) => {
              const toolError = remoteToolError(cause)
              if (toolError?.tag === "custom-error") {
                const declared = model.body!.errors.find(
                  (entry) => entry.name === toolError.val.name,
                )
                if (!declared)
                  return Effect.fail(
                    new ToolClientError("declared-error", {
                      tag: "unknown-error",
                      name: toolError.val.name,
                      payload: toolError.val.payload,
                    }),
                  )
                return Effect.gen(function* () {
                  const codec = yield* compile(declared.schema).pipe(
                    Effect.mapError((error) => new ToolClientError("declared-error", error)),
                  )
                  if (!sameWireGraph(codec.schemaGraph, toolError.val.payload.graph))
                    return yield* Effect.fail(
                      new ToolClientError(
                        "declared-error",
                        `custom error '${declared.name}' schema does not match`,
                      ),
                    )
                  const value = yield* codec
                    .decode(toolError.val.payload.value)
                    .pipe(Effect.mapError((error) => new ToolClientError("declared-error", error)))
                  return yield* Effect.fail({
                    _tag: "ToolFailure",
                    name: declared.name,
                    value,
                  } as const)
                })
              }
              return Effect.fail(new ToolClientError("invoke", cause))
            },
          ),
        )
        const decodedResult = invocationResult.pipe(
          Effect.flatMap((result) => {
            if (!model.body!.output)
              return result.result === undefined
                ? Effect.succeed(undefined)
                : Effect.fail(new ToolClientError("result", "unexpected remote result"))
            if (!result.result) return Effect.fail(new ToolClientError("result", "missing result"))
            return compile(model.body!.output).pipe(
              Effect.mapError((cause) => new ToolClientError("result", cause)),
              Effect.flatMap((output) => {
                if (!sameWireGraph(output.schemaGraph, result.result!.graph))
                  return Effect.fail(new ToolClientError("result", "result schema does not match"))
                return output
                  .decode(result.result!.value)
                  .pipe(Effect.mapError((cause) => new ToolClientError("result", cause)))
              }),
            )
          }),
        )
        const normalizedConsume = consume.pipe(
          Effect.mapError((cause) =>
            cause instanceof ToolClientError ? cause : new ToolClientError("stream", cause),
          ),
        )
        const [consumeExit, resultExit] = yield* Effect.all(
          [Effect.exit(normalizedConsume), Effect.exit(decodedResult)] as const,
          {
            concurrency: "unbounded",
          },
        )
        if (Exit.isFailure(resultExit)) return yield* Effect.failCause(resultExit.cause)
        if (Exit.isFailure(consumeExit)) return yield* Effect.failCause(consumeExit.cause)
        return resultExit.value
      }),
    )

  const build = (model: CommandModel, path: readonly string[]): any => {
    const node: any = model.body
      ? (input: Record<string, unknown>, streams?: Streams) => call(model, path, input, streams)
      : {}
    for (const child of Object.values(model.children)) {
      node[child.name.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())] = build(child, [
        ...path,
        child.name,
      ])
    }
    return node
  }
  return build(definition.model, [])
}

registerToolClientFactory(client)

/** A caller-owned typed subset of a remote tool's commands. @since 1.6.0 @category models */
export interface ToolClientDefinition<D extends ToolDefinition<any, any>> {
  readonly name?: string
  readonly definition: D
  readonly client: (
    targetName?: string,
    options?: Omit<ClientOptions, "lookupName">,
  ) => Client<D, ToolClient>
}

/** Bind a typed command subset optimistically, without discovery. @since 1.6.0 @category constructors */
export const toolClientDefinition = <D extends ToolDefinition<any, any>>(
  definition: D,
  name?: string,
): ToolClientDefinition<D> =>
  Object.freeze({
    name,
    definition,
    client: (targetName?: string, options: Omit<ClientOptions, "lookupName"> = {}) => {
      const lookupName = name ?? targetName
      if (!lookupName)
        throw new TypeError("a nameless tool client definition requires a target name")
      return client(definition, { ...options, lookupName })
    },
  })

const isToolError = (value: unknown): value is Common.ToolError =>
  typeof value === "object" && value !== null && "tag" in value
const remoteToolError = (value: unknown): Common.ToolError | undefined => {
  if (typeof value !== "object" || value === null || !("tag" in value)) return undefined
  const tagged = value as { readonly tag: unknown; readonly val?: unknown }
  if (tagged.tag === "custom-error" && isToolError(value)) return value
  if (tagged.tag !== "remote-tool-error") return undefined
  return isToolError(tagged.val) ? tagged.val : undefined
}
const sameWireGraph = (left: Common.SchemaGraph, right: Common.SchemaGraph): boolean => {
  try {
    return schemaShapesMatch(schemaGraphFromWit(left), schemaGraphFromWit(right))
  } catch {
    return false
  }
}
