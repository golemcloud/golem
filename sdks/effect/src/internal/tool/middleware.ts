import type * as Common from "golem:tool/common@0.1.0"
import type * as UnderlyingWit from "golem:tool/underlying@0.1.0"
import type * as StreamsWit from "golem:tool/streams@0.1.0"
import type * as Agent from "golem:agent/common@2.0.0"
import { Context, Effect, Exit, Layer, Schema, Scope, Stream } from "effect"
import { AbortableStreamIterable } from "../abortableStreamIterable.js"
import { schemaShapesMatch } from "../schema-model/model.js"
import { schemaGraphFromWit } from "../schema-model/wit.js"
import { type CompiledWitCodec, compile } from "../../WitCodec.js"
import {
  type BodyModel,
  type CompiledBody,
  type CommandError,
  type CommandInput,
  type CommandBuilder,
  type CommandModel,
  type CommandOutput,
  compileDefinition,
  err,
  type ToolDefinition,
  type ToolFailure,
  type ToolImplementation,
} from "./model.js"

/** Middleware invocation error. @since 1.6.0 @category errors */
type MiddlewareErrorCause =
  | Common.ToolError
  | Exclude<UnderlyingWit.UnderlyingError, { tag: "tool-error" }>

export class MiddlewareError extends Error {
  readonly _tag = "MiddlewareError"
  constructor(readonly cause: MiddlewareErrorCause) {
    super(`Tool middleware failed: ${cause.tag}`)
  }
}

/** A started underlying call whose terminal and stdout can be observed independently. @since 1.6.0 @category models */
export interface StartedInvocation {
  readonly get: Effect.Effect<Common.InvocationResult, MiddlewareError>
  readonly cancel: Effect.Effect<void>
  readonly stdout?: Stream.Stream<Uint8Array, MiddlewareError>
}

/** Runtime-provided affine access to the next chain layer. @since 1.6.0 @category models */
export interface Underlying {
  readonly start: (
    path: readonly string[],
    input: Common.TypedSchemaValue,
    stdin?: Stream.Stream<Uint8Array, MiddlewareError>,
  ) => Effect.Effect<StartedInvocation, MiddlewareError, Scope.Scope>
  readonly invoke: (
    path: readonly string[],
    input: Common.TypedSchemaValue,
    stdin?: Stream.Stream<Uint8Array, MiddlewareError>,
  ) => Effect.Effect<Common.InvocationResult, MiddlewareError>
}

/** Typed streams accepted by an underlying command. @since 1.6.0 @category models */
export interface TypedStreams<R = never> {
  readonly stdin?: Stream.Stream<Uint8Array, MiddlewareError>
  readonly stdout?: (
    stream: Stream.Stream<Uint8Array, MiddlewareError>,
  ) => Effect.Effect<void, unknown, R>
}

type TypedUnderlyingNode<M extends CommandModel> = (M extends {
  readonly body: infer B extends BodyModel
}
  ? (<R = never>(
      input: CommandInput<B>,
      streams?: TypedStreams<R>,
    ) => Effect.Effect<CommandOutput<B>, MiddlewareError | CommandError<B>, R>) & {
      readonly start: (
        input: CommandInput<B>,
        stdin?: Stream.Stream<Uint8Array, MiddlewareError>,
      ) => Effect.Effect<
        TypedStartedInvocation<CommandOutput<B>, CommandError<B>>,
        MiddlewareError,
        Scope.Scope
      >
    }
  : object) & {
  readonly [K in keyof M["children"] as CamelCase<string & K>]: TypedUnderlyingNode<
    M["children"][K]
  >
}

/** A definition-derived started underlying call. @since 1.6.0 @category models */
export interface TypedStartedInvocation<Output, Error> {
  readonly get: Effect.Effect<Output, MiddlewareError | Error>
  readonly cancel: Effect.Effect<void>
  readonly stdout?: Stream.Stream<Uint8Array, MiddlewareError>
}
type CamelCase<S extends string> = S extends `${infer H}-${infer T}`
  ? `${H}${Capitalize<CamelCase<T>>}`
  : S
type DefinitionModel<D> =
  D extends CommandBuilder<infer M> ? M : D extends ToolDefinition<any, infer M> ? M : never

/** Definition-derived access to the wrapped tool. @since 1.6.0 @category models */
export type TypedUnderlying<D extends ToolDefinition<any, any>> = TypedUnderlyingNode<
  DefinitionModel<D>
>

/** Context passed to a definition-derived middleware command. @since 1.6.0 @category models */
export interface TypedHandlerContext<D extends ToolDefinition<any, any>, Parameters> {
  readonly principal: Agent.Principal
  readonly parameters: Parameters
  readonly stdin?: Stream.Stream<Uint8Array, MiddlewareError>
  readonly stdout?: <R2>(
    stream: Stream.Stream<Uint8Array, MiddlewareError, R2>,
  ) => Effect.Effect<void, never, R2>
  readonly underlying: TypedUnderlying<D>
}

type TypedHandler<B extends BodyModel, D extends ToolDefinition<any, any>, Parameters, R> = (
  input: CommandInput<B>,
  context: TypedHandlerContext<D, Parameters>,
) => Effect.Effect<CommandOutput<B> | CommandError<B>, MiddlewareError | CommandError<B>, R>
type TypedImplementationNode<
  M extends CommandModel,
  D extends ToolDefinition<any, any>,
  Parameters,
  R,
> = (M extends { readonly body: infer B extends BodyModel }
  ? TypedHandler<B, D, Parameters, R>
  : object) & {
  readonly [K in keyof M["children"] as CamelCase<string & K>]: TypedImplementationNode<
    M["children"][K],
    D,
    Parameters,
    R
  >
}
/** Command implementation projected from presented and wrapped definitions. @since 1.6.0 @category models */
export type TypedImplementation<
  P extends ToolDefinition<any, any>,
  D extends ToolDefinition<any, any> = P,
  Parameters = Record<string, never>,
  R = never,
> = {
  readonly [K in P["name"] as CamelCase<string & K>]: TypedImplementationNode<
    DefinitionModel<P>,
    D,
    Parameters,
    R
  >
}

/** Universal middleware invocation metadata. @since 1.6.0 @category models */
export interface MiddlewareInvocation<Parameters> {
  readonly toolName: string
  readonly tool: Common.Tool
  readonly commandPath: readonly string[]
  readonly input: Common.TypedSchemaValue
  readonly stdin?: Stream.Stream<Uint8Array, MiddlewareError>
  readonly principal: Agent.Principal
  readonly parameters: Parameters
}

/** Universal middleware handler. @since 1.6.0 @category models */
export type UniversalHandler<Parameters, R = never> = (
  invocation: MiddlewareInvocation<Parameters>,
  underlying: Underlying,
) => Effect.Effect<Common.InvocationResult, MiddlewareError, R>

/** Universal middleware declaration. @since 1.6.0 @category models */
export interface MiddlewareOptions<N extends string, S extends Schema.Top, R = never> {
  readonly name: N
  readonly version?: string
  readonly aliases?: readonly string[]
  readonly doc?: string | Partial<Common.Doc>
  readonly parameters: S
  readonly handler: UniversalHandler<S["Type"], R>
  readonly layer?: Layer.Layer<R>
}

/** Registered middleware handle. @since 1.6.0 @category models */
export interface ImplementedMiddleware<N extends string = string> {
  readonly name: N
}

interface Entry {
  readonly wire: Common.ToolMiddleware
  readonly parameters: CompiledWitCodec<Schema.Top>
  readonly handler: UniversalHandler<any, any>
  readonly layer?: Layer.Layer<any>
}
const entries = new Map<string, Entry>()
const normalizeDoc = (d: MiddlewareOptions<string, Schema.Top>["doc"]): Common.Doc =>
  typeof d === "string"
    ? { summary: d, description: "", examples: [] }
    : { summary: d?.summary ?? "", description: d?.description ?? "", examples: d?.examples ?? [] }

const forbiddenParameterTypes = new Set([
  "future",
  "stream",
  "secret",
  "quota-token",
  "permission-card",
])

const assertStaticParameterSchema = (parameters: CompiledWitCodec<Schema.Top>): void => {
  const visit = (value: unknown): void => {
    if (typeof value !== "object" || value === null) return
    if (value instanceof Map) {
      for (const entry of value.values()) visit(entry)
      return
    }
    const tag = (value as { readonly tag?: unknown }).tag
    if (typeof tag === "string" && forbiddenParameterTypes.has(tag))
      throw new Error(`Middleware installation parameters cannot contain ${tag}`)
    for (const child of Object.values(value)) visit(child)
  }
  visit(parameters.graph)
}

const register = <N extends string, S extends Schema.Top, R>(
  options: MiddlewareOptions<N, S, R>,
  scope: Common.ToolMiddlewareScope,
): ImplementedMiddleware<N> => {
  if (entries.has(options.name))
    throw new Error(`Middleware '${options.name}' is already registered`)
  const parameters = Effect.runSync(
    compile(options.parameters),
  ) as unknown as CompiledWitCodec<Schema.Top>
  assertStaticParameterSchema(parameters)
  entries.set(options.name, {
    wire: {
      name: options.name,
      version: options.version ?? "0.0.0",
      aliases: [...(options.aliases ?? [])],
      doc: normalizeDoc(options.doc),
      scope,
      parameterSchema: parameters.schemaGraph,
    },
    parameters,
    handler: options.handler,
    layer: options.layer,
  })
  return { name: options.name }
}

/** Define universal transparent middleware. @since 1.6.0 @category constructors */
export const universal = <N extends string, S extends Schema.Top, R = never>(
  options: MiddlewareOptions<N, S, R>,
) => register(options, { tag: "universal" })

/** Empty installation parameters for middleware that has no configuration. @since 1.6.0 @category schemas */
export const NoParameters = Schema.Struct({})

/** Define typed middleware projected from presented and expected tool definitions. @since 1.6.0 @category constructors */
export function typed<
  N extends string,
  P extends ToolDefinition<any, any>,
  D extends ToolDefinition<any, any>,
  S extends Schema.Top,
  R = never,
>(
  options: Omit<MiddlewareOptions<N, S, R>, "handler"> & {
    readonly presented: P
    readonly expected: D
    readonly handler: TypedImplementation<P, D, S["Type"], R>
  },
): ImplementedMiddleware<N>
export function typed<
  N extends string,
  P extends ToolDefinition<any, any>,
  S extends Schema.Top,
  R = never,
>(
  options: Omit<MiddlewareOptions<N, S, R>, "handler"> & {
    readonly presented: P
    readonly expected?: undefined
    readonly handler: TypedImplementation<P, P, S["Type"], R>
  },
): ImplementedMiddleware<N>
export function typed<N extends string, S extends Schema.Top, R = never>(
  options: Omit<MiddlewareOptions<N, S, R>, "handler"> & {
    readonly presented: ToolDefinition<any, any>
    readonly expected?: ToolDefinition<any, any>
    readonly handler: ToolImplementation
  },
): ImplementedMiddleware<N> {
  const presented = compileDefinition(options.presented)
  const expected = options.expected ? compileDefinition(options.expected) : presented
  assertCompleteImplementation(
    presented.definition.model,
    options.handler,
    presented.definition.name,
  )
  const handler = typedHandler(presented, expected, options.handler)
  return register(
    { ...options, handler },
    {
      tag: "monomorphic",
      val: { presented: presented.wire, expected: expected.wire },
    },
  )
}

const camelCase = (name: string) => name.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())

const assertCompleteImplementation = (
  model: CommandModel,
  implementation: ToolImplementation,
  rootName: string,
): void => {
  const root = implementation[camelCase(rootName)]
  const visit = (command: CommandModel, node: unknown, path: readonly string[]): void => {
    if (command.body && typeof node !== "function")
      throw new Error(`Middleware implementation is missing handler '${path.join("/")}'`)
    for (const child of Object.values(command.children)) {
      const childNode =
        (typeof node === "object" && node !== null) || typeof node === "function"
          ? (node as ToolImplementation)[camelCase(child.name)]
          : undefined
      visit(child, childNode, [...path, child.name])
    }
  }
  visit(model, root, [])
}

export const resetMiddlewares = () => entries.clear()

const typedHandler =
  (
    presented: ReturnType<typeof compileDefinition>,
    expected: ReturnType<typeof compileDefinition>,
    implementation: ToolImplementation,
  ): UniversalHandler<any, any> =>
  (invocation, rawUnderlying) =>
    Effect.gen(function* () {
      const body = presented.bodies.get(invocation.commandPath.join("/"))
      let commandHandler: unknown = implementation[camelCase(presented.definition.name)]
      for (const segment of invocation.commandPath)
        commandHandler =
          (typeof commandHandler === "object" && commandHandler !== null) ||
          typeof commandHandler === "function"
            ? (commandHandler as ToolImplementation)[camelCase(segment)]
            : undefined
      if (!body || !commandHandler)
        return yield* Effect.fail(
          new MiddlewareError({ tag: "invalid-command-path", val: [...invocation.commandPath] }),
        )
      const input = yield* body.input
        .decode(invocation.input.value)
        .pipe(
          Effect.mapError(
            (cause) => new MiddlewareError({ tag: "invalid-input", val: String(cause) }),
          ),
        )
      if (!body.model.stdin && invocation.stdin)
        return yield* Effect.fail(
          new MiddlewareError({ tag: "invalid-input", val: "undeclared stdin stream" }),
        )
      if (body.model.stdin?.required && !invocation.stdin)
        return yield* Effect.fail(
          new MiddlewareError({ tag: "invalid-input", val: "required stdin stream is missing" }),
        )
      let stdout:
        | {
            readonly stream: Stream.Stream<Uint8Array, MiddlewareError, any>
            readonly context: Context.Context<any>
          }
        | undefined
      const result = yield* (commandHandler as any)(input, {
        principal: invocation.principal,
        parameters: invocation.parameters,
        stdin: invocation.stdin,
        stdout: body.model.stdout
          ? (stream: Stream.Stream<Uint8Array, MiddlewareError, any>) =>
              Effect.gen(function* () {
                if (stdout)
                  throw new MiddlewareError({
                    tag: "invalid-result",
                    val: "middleware stdout was supplied more than once",
                  })
                stdout = { stream, context: yield* Effect.context<any>() }
              })
          : undefined,
        underlying: buildUnderlying(expected, rawUnderlying),
      }).pipe(
        Effect.catchIf(
          (_cause: unknown): _cause is unknown => true,
          (cause: unknown) =>
            isFailure(cause)
              ? Effect.succeed(cause)
              : cause instanceof MiddlewareError
                ? Effect.fail(cause)
                : Effect.die(cause),
        ),
      )
      if (isFailure(result)) {
        const declared = body.errors.find((entry) => entry.spec.name === result.name)
        if (!declared)
          return yield* Effect.fail(
            new MiddlewareError({
              tag: "invalid-result",
              val: `undeclared error '${result.name}'`,
            }),
          )
        const value = yield* declared.codec
          .encodeAsync(result.value)
          .pipe(
            Effect.mapError(
              (cause) => new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
            ),
          )
        return yield* Effect.fail(
          new MiddlewareError({
            tag: "custom-error",
            val: {
              name: declared.spec.name,
              payload: { graph: declared.codec.schemaGraph, value },
            },
          }),
        )
      }
      if (body.model.stdout?.required && !stdout)
        return yield* Effect.fail(
          new MiddlewareError({ tag: "invalid-result", val: "required stdout stream is missing" }),
        )
      const projectedStdout = stdout ? outputStreamWith(stdout.stream, stdout.context) : undefined
      if (!body.output) {
        if (result !== undefined)
          return yield* Effect.fail(
            new MiddlewareError({ tag: "invalid-result", val: "unexpected structured result" }),
          )
        return { result: undefined, stdout: projectedStdout }
      }
      const value = yield* body.output
        .encodeAsync(result)
        .pipe(
          Effect.mapError(
            (cause) => new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
          ),
        )
      return { result: { graph: body.output.schemaGraph, value }, stdout: projectedStdout }
    }) as Effect.Effect<Common.InvocationResult, MiddlewareError, any>

const isFailure = (value: unknown): value is ToolFailure<string, unknown> =>
  typeof value === "object" &&
  value !== null &&
  (value as { _tag?: unknown })._tag === "ToolFailure"

const buildUnderlying = (compiled: ReturnType<typeof compileDefinition>, raw: Underlying): any => {
  const build = (model: CommandModel, path: readonly string[]): any => {
    const body = compiled.bodies.get(path.join("/"))
    const node: any = body
      ? Object.assign(
          (input: Record<string, unknown>, streams?: TypedStreams) =>
            Effect.scoped(
              Effect.gen(function* () {
                const started = yield* node.start(input, streams?.stdin)
                const consume = started.stdout
                  ? streams?.stdout
                    ? streams.stdout(started.stdout)
                    : Stream.runDrain(started.stdout)
                  : Effect.void
                const [result] = yield* Effect.all([started.get, consume], {
                  concurrency: "unbounded",
                })
                return result
              }),
            ),
          {
            start: (
              input: Record<string, unknown>,
              stdin?: Stream.Stream<Uint8Array, MiddlewareError>,
            ) =>
              Effect.gen(function* () {
                if (!body.model.stdin && stdin)
                  return yield* Effect.fail(
                    new MiddlewareError({ tag: "invalid-input", val: "undeclared stdin stream" }),
                  )
                if (body.model.stdin?.required && !stdin)
                  return yield* Effect.fail(
                    new MiddlewareError({
                      tag: "invalid-input",
                      val: "required stdin stream is missing",
                    }),
                  )
                const value = yield* body.input
                  .encodeAsync(input)
                  .pipe(
                    Effect.mapError(
                      (cause) => new MiddlewareError({ tag: "invalid-input", val: String(cause) }),
                    ),
                  )
                const invocation = yield* raw.start(
                  path,
                  { graph: body.input.schemaGraph, value },
                  stdin,
                )
                if (body.model.stdout?.required && !invocation.stdout)
                  return yield* Effect.fail(
                    new MiddlewareError({
                      tag: "invalid-result",
                      val: "required stdout stream is missing",
                    }),
                  )
                const get = invocation.get.pipe(
                  Effect.catchTag("MiddlewareError", (error) => decodeUnderlyingError(body, error)),
                  Effect.flatMap((success) => {
                    if (!body.output) return Effect.succeed(undefined)
                    if (!success.result)
                      return Effect.fail(
                        new MiddlewareError({ tag: "invalid-result", val: "missing result" }),
                      )
                    return body.output
                      .decode(success.result.value)
                      .pipe(
                        Effect.mapError(
                          (cause) =>
                            new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
                        ),
                      )
                  }),
                )
                return { get, cancel: invocation.cancel, stdout: invocation.stdout }
              }),
          },
        )
      : {}
    for (const child of Object.values(model.children))
      node[camelCase(child.name)] = build(child, [...path, child.name])
    return node
  }
  return build(compiled.definition.model, [])
}

const decodeUnderlyingError = (body: CompiledBody, error: MiddlewareError) => {
  const custom = customToolError(error.cause)
  if (!custom) return Effect.fail(error)
  const declared = body.errors.find((entry) => entry.spec.name === custom.val.name)
  if (!declared || !sameWireGraph(declared.codec.schemaGraph, custom.val.payload.graph))
    return Effect.fail(error)
  return declared.codec.decode(custom.val.payload.value).pipe(
    Effect.flatMap((decoded) => Effect.fail(err(declared.spec.name, decoded))),
    Effect.mapError((cause) =>
      isFailure(cause) ? cause : new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
    ),
  )
}

const customToolError = (
  cause: MiddlewareErrorCause,
): Extract<Common.ToolError, { tag: "custom-error" }> | undefined => {
  const tagged = cause as Common.ToolError | { tag: "remote-tool-error"; val: Common.ToolError }
  const error = tagged.tag === "remote-tool-error" ? tagged.val : tagged
  return error.tag === "custom-error" ? error : undefined
}

const sameWireGraph = (
  left: Common.TypedSchemaValue["graph"],
  right: Common.TypedSchemaValue["graph"],
): boolean => {
  try {
    return schemaShapesMatch(schemaGraphFromWit(left), schemaGraphFromWit(right))
  } catch {
    return false
  }
}

const inputStream = (source: AsyncIterable<number> | undefined) =>
  source === undefined
    ? undefined
    : Stream.fromAsyncIterable(
        source,
        (cause) => new MiddlewareError({ tag: "invalid-input", val: String(cause) }),
      ).pipe(Stream.map((byte) => Uint8Array.of(byte)))

const outputStreamWith = <R>(
  source: Stream.Stream<Uint8Array, MiddlewareError, R>,
  context: Context.Context<R>,
) =>
  new AbortableStreamIterable(
    Stream.flatMap(source, (chunk) => Stream.fromIterable(chunk)),
    context,
  )

class InvocationOwnership {
  private readonly owned: ManagedAsyncIterable[] = []
  private disposed = false

  own(stream: AsyncIterable<number>): ManagedAsyncIterable {
    const managed = new ManagedAsyncIterable(stream)
    this.owned.push(managed)
    if (this.disposed) void managed.close().catch(() => undefined)
    return managed
  }

  async dispose(): Promise<void> {
    this.disposed = true
    const results = await Promise.allSettled(this.owned.map((stream) => stream.close()))
    const failure = results.find(
      (result): result is PromiseRejectedResult => result.status === "rejected",
    )
    if (failure) throw failure.reason
  }
}

class ManagedAsyncIterable implements AsyncIterableIterator<number> {
  private iterator: AsyncIterator<number> | undefined
  private closePromise: Promise<void> | undefined

  constructor(private readonly source: AsyncIterable<number>) {}

  [Symbol.asyncIterator](): AsyncIterableIterator<number> {
    return this
  }

  next(): Promise<IteratorResult<number>> {
    if (this.closePromise) return Promise.resolve({ done: true, value: undefined })
    return this.getIterator().next()
  }

  return(): Promise<IteratorResult<number>> {
    return this.close().then(() => ({ done: true, value: undefined }))
  }

  close(): Promise<void> {
    return (this.closePromise ??= Promise.resolve()
      .then(() => this.getIterator().return?.())
      .then(() => undefined))
  }

  private getIterator(): AsyncIterator<number> {
    return (this.iterator ??= this.source[Symbol.asyncIterator]())
  }
}

export const toolMiddlewareGuest = {
  discoverToolMiddlewares: (): Common.ToolMiddleware[] => [...entries.values()].map((x) => x.wire),
  getToolMiddleware: (name: string): Common.ToolMiddleware => {
    const entry = entries.get(name)
    if (!entry) throw { tag: "invalid-tool-name", val: name } satisfies Common.ToolError
    return entry.wire
  },
  invokeToolMiddleware: async (
    middlewareName: string,
    toolName: string,
    tool: Common.Tool,
    parameters: Common.TypedSchemaValue,
    commandPath: string[],
    input: Common.TypedSchemaValue,
    stdin: AsyncIterable<StreamsWit.ByteStreamItem> | undefined,
    stdout: StreamsWit.ToolStdoutWriter | undefined,
    principal: Agent.Principal,
    wrapped: UnderlyingWit.UnderlyingTool,
  ): Promise<Common.InvocationResult> => {
    const entry = entries.get(middlewareName)
    if (!entry) throw { tag: "invalid-tool-name", val: middlewareName } satisfies Common.ToolError
    const ownership = new InvocationOwnership()
    const managedStdin = stdin ? ownership.own(decodeByteStream(stdin)) : undefined
    const middlewareStdin = inputStream(managedStdin)
    const invocationScope = await Effect.runPromise(Scope.make())
    let active = true
    const admitted = new Set<Promise<unknown>>()
    const observers = new Set<ObserverLease>()
    const releaseObserver = (lease: ObserverLease) => {
      if (observers.delete(lease)) lease.release()
    }
    const underlying: Underlying = {
      start: (path, value, source) =>
        Effect.gen(function* () {
          if (!active) {
            return yield* Effect.fail(
              new MiddlewareError({
                tag: "invalid-input",
                val: "underlying tool used after invocation",
              }),
            )
          }
          const iterable = source
            ? ownership.own(outputStreamWith(source, yield* Effect.context<never>()))
            : undefined
          const admission = Promise.resolve()
            .then(() =>
              wrapped.invoke([...path], value, iterable ? encodeByteStream(iterable) : undefined),
            )
            .then(([observer, output]) => {
              const lease = new ObserverLease(observer)
              observers.add(lease)
              return {
                lease,
                output: output ? ownership.own(decodeByteStream(output)) : undefined,
              }
            })
          admitted.add(admission)
          void admission.then(
            () => admitted.delete(admission),
            () => admitted.delete(admission),
          )
          const admittedCall = yield* Effect.tryPromise({
            try: () => admission,
            catch: underlyingError,
          })
          const { lease, output } = admittedCall
          yield* Effect.addFinalizer(() => Effect.sync(() => releaseObserver(lease)))
          return {
            get: Effect.tryPromise({
              try: async () => ({ result: await lease.observe() }),
              catch: underlyingError,
            }),
            cancel: Effect.sync(() => lease.cancel()),
            stdout: inputStream(output),
          }
        }),
      invoke: (path, value, source) =>
        Effect.scoped(
          Effect.gen(function* () {
            const started = yield* underlying.start(path, value, source)
            const consume = started.stdout ? Stream.runDrain(started.stdout) : Effect.void
            const [result] = yield* Effect.all([started.get, consume], {
              concurrency: "unbounded",
            })
            return result
          }),
        ),
    }
    const effect = Effect.gen(function* () {
      if (!sameWireGraph(entry.parameters.schemaGraph, parameters.graph))
        return yield* Effect.fail(
          new MiddlewareError({
            tag: "invalid-input",
            val: "installation parameters do not match the declared schema",
          }),
        )
      const decodedParameters = yield* entry.parameters.decode(parameters.value).pipe(
        Effect.mapError(
          (cause) =>
            new MiddlewareError({
              tag: "invalid-input",
              val: `invalid installation parameters: ${String(cause)}`,
            }),
        ),
      )
      return yield* entry.handler(
        {
          toolName,
          tool,
          commandPath,
          input,
          stdin: middlewareStdin,
          principal,
          parameters: decodedParameters,
        },
        underlying,
      )
    })
    const closeLayer = () => Effect.runPromise(Scope.close(invocationScope, Exit.void))
    let cleanupPromise: Promise<void> | undefined
    const cleanup = (primary?: unknown): Promise<void> =>
      (cleanupPromise ??= (async () => {
        await Promise.allSettled(admitted)
        const ownershipResult = await Promise.allSettled([ownership.dispose()])
        for (const observer of observers) releaseObserver(observer)
        const layerResult = await Promise.allSettled([closeLayer()])
        if (primary !== undefined) throw primary
        const failure = [...ownershipResult, ...layerResult].find(
          (result): result is PromiseRejectedResult => result.status === "rejected",
        )
        if (failure) throw failure.reason
      })())
    let context = Context.empty()
    try {
      if (entry.layer)
        context = await Effect.runPromise(Layer.buildWithScope(entry.layer, invocationScope))
      const result = await Effect.runPromise(
        Effect.provide(effect as Effect.Effect<Common.InvocationResult, MiddlewareError>, context),
      )
      active = false
      const output = result.stdout ? ownership.own(result.stdout) : undefined
      try {
        if (output && stdout) {
          for (;;) {
            const next = await output.next()
            if (next.done) break
            await stdout.write(Uint8Array.of(next.value))
          }
        }
        if (stdout) await stdout.finish()
      } catch (cause) {
        if (stdout)
          await stdout
            .fail({
              tag: "failed",
              val: cause instanceof Error ? cause.message : String(cause),
            })
            .catch(() => undefined)
        throw cause
      }
      await Promise.allSettled(admitted)
      await cleanup()
      return { result: result.result }
    } catch (cause) {
      active = false
      await cleanup(cause instanceof MiddlewareError ? exportMiddlewareError(cause.cause) : cause)
      throw cause
    }
  },
}

const underlyingError = (cause: unknown): MiddlewareError => {
  const error = cause as UnderlyingWit.UnderlyingError
  switch (error?.tag) {
    case "tool-error":
      return new MiddlewareError(error.val)
    case "protocol-error":
    case "denied":
    case "internal-error":
    case "cancelled":
    case "resource-exhausted":
      return new MiddlewareError(error)
    default:
      return new MiddlewareError(
        isToolError(cause)
          ? cause
          : {
              tag: "invalid-result",
              val: `underlying invocation failed: ${String(cause)}`,
            },
      )
  }
}

const exportMiddlewareError = (cause: MiddlewareErrorCause): Common.ToolError => {
  switch (cause.tag) {
    case "protocol-error":
    case "internal-error":
      return { tag: "invalid-result", val: `${cause.tag}: ${cause.val}` }
    case "denied":
      return { tag: "constraint-violation", val: cause.val }
    case "cancelled":
      return { tag: "constraint-violation", val: "underlying invocation was cancelled" }
    case "resource-exhausted":
      return { tag: "constraint-violation", val: cause.val }
    default:
      return cause
  }
}

const isToolError = (value: unknown): value is Common.ToolError =>
  typeof value === "object" &&
  value !== null &&
  typeof (value as { tag?: unknown }).tag === "string"

class ObserverLease {
  private result: Promise<Common.TypedSchemaValue | undefined> | undefined
  private settled = false
  private released = false
  private disposed = false

  constructor(private readonly observer: UnderlyingWit.UnderlyingInvokeResult) {}

  observe(): Promise<Common.TypedSchemaValue | undefined> {
    if (this.result !== undefined) return this.result
    if (this.released) {
      return Promise.reject({
        tag: "protocol-error",
        val: "underlying invocation observer was released",
      })
    }
    return (this.result ??= this.observer.get().finally(() => {
      this.settled = true
      if (this.released) this.dispose()
    }))
  }

  cancel(): void {
    if (!this.released) this.observer.cancel()
  }

  release(): void {
    this.released = true
    if (this.result === undefined || this.settled) this.dispose()
  }

  private dispose(): void {
    if (this.disposed) return
    this.disposed = true
    const disposable = this.observer as UnderlyingWit.UnderlyingInvokeResult & {
      [Symbol.dispose]?: () => void
    }
    disposable[Symbol.dispose]?.()
  }
}

function decodeByteStream(source: AsyncIterable<StreamsWit.ByteStreamItem>): AsyncIterable<number> {
  const iterator = source[Symbol.asyncIterator]()
  let chunk: Iterator<number> | undefined
  return {
    [Symbol.asyncIterator]() {
      return {
        async next(): Promise<IteratorResult<number>> {
          for (;;) {
            const byte = chunk?.next()
            if (byte && !byte.done) return byte
            const next = await iterator.next()
            if (next.done) return { done: true, value: undefined }
            if (next.value.tag === "err") throw next.value.val
            chunk = next.value.val[Symbol.iterator]()
          }
        },
        async return(): Promise<IteratorResult<number>> {
          await iterator.return?.()
          return { done: true, value: undefined }
        },
      }
    },
  }
}

async function* encodeByteStream(
  source: AsyncIterable<number>,
): AsyncIterable<StreamsWit.ByteStreamItem> {
  for await (const byte of source) yield { tag: "ok", val: Uint8Array.of(byte) }
}
