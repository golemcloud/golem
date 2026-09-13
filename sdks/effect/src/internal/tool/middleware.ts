import type * as Common from "golem:tool/common@0.1.0"
import type * as Agent from "golem:agent/common@2.0.0"
import { Context, Effect, Exit, Layer, Scope, Stream } from "effect"
import { AbortableStreamIterable } from "../abortableStreamIterable.js"
import { compile } from "../../WitCodec.js"
import {
  type BodyModel,
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
export class MiddlewareError extends Error {
  readonly _tag = "MiddlewareError"
  constructor(readonly cause: Common.ToolError) {
    super(`Tool middleware failed: ${cause.tag}`)
  }
}

/** Runtime-provided affine access to the next chain layer. @since 1.6.0 @category models */
export interface Underlying {
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
  ? <R = never>(
      input: CommandInput<B>,
      streams?: TypedStreams<R>,
    ) => Effect.Effect<CommandOutput<B>, MiddlewareError | CommandError<B>, R>
  : object) & {
  readonly [K in keyof M["children"] as CamelCase<string & K>]: TypedUnderlyingNode<
    M["children"][K]
  >
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
export interface TypedHandlerContext<D extends ToolDefinition<any, any>> {
  readonly principal: Agent.Principal
  readonly stdin?: Stream.Stream<Uint8Array, MiddlewareError>
  readonly stdout?: <R2>(
    stream: Stream.Stream<Uint8Array, MiddlewareError, R2>,
  ) => Effect.Effect<void, never, R2>
  readonly underlying: TypedUnderlying<D>
}

type TypedHandler<B extends BodyModel, D extends ToolDefinition<any, any>, R> = (
  input: CommandInput<B>,
  context: TypedHandlerContext<D>,
) => Effect.Effect<CommandOutput<B> | CommandError<B>, MiddlewareError | CommandError<B>, R>
type TypedImplementationNode<
  M extends CommandModel,
  D extends ToolDefinition<any, any>,
  R,
> = (M extends { readonly body: infer B extends BodyModel } ? TypedHandler<B, D, R> : object) & {
  readonly [K in keyof M["children"] as CamelCase<string & K>]: TypedImplementationNode<
    M["children"][K],
    D,
    R
  >
}
/** Command implementation projected from presented and wrapped definitions. @since 1.6.0 @category models */
export type TypedImplementation<
  P extends ToolDefinition<any, any>,
  D extends ToolDefinition<any, any> = P,
  R = never,
> = {
  readonly [K in P["name"] as CamelCase<string & K>]: TypedImplementationNode<
    DefinitionModel<P>,
    D,
    R
  >
}

/** Universal middleware invocation metadata. @since 1.6.0 @category models */
export interface MiddlewareInvocation {
  readonly toolName: string
  readonly tool: Common.Tool
  readonly commandPath: readonly string[]
  readonly input: Common.TypedSchemaValue
  readonly stdin?: Stream.Stream<Uint8Array, MiddlewareError>
  readonly principal: Agent.Principal
}

/** Universal middleware handler. @since 1.6.0 @category models */
export type UniversalHandler<R = never> = (
  invocation: MiddlewareInvocation,
  underlying: Underlying,
) => Effect.Effect<Common.InvocationResult, MiddlewareError, R>

/** Universal middleware declaration. @since 1.6.0 @category models */
export interface MiddlewareOptions<N extends string, R = never> {
  readonly name: N
  readonly aliases?: readonly string[]
  readonly doc?: string | Partial<Common.Doc>
  readonly handler: UniversalHandler<R>
  readonly layer?: Layer.Layer<R>
}

/** Registered middleware handle. @since 1.6.0 @category models */
export interface ImplementedMiddleware<N extends string = string> {
  readonly name: N
}

interface Entry {
  readonly wire: Common.ToolMiddleware
  readonly handler: UniversalHandler<any>
  readonly layer?: Layer.Layer<any>
}
const entries = new Map<string, Entry>()
const normalizeDoc = (d: MiddlewareOptions<string>["doc"]): Common.Doc =>
  typeof d === "string"
    ? { summary: d, description: "", examples: [] }
    : { summary: d?.summary ?? "", description: d?.description ?? "", examples: d?.examples ?? [] }

const register = <N extends string, R>(
  options: MiddlewareOptions<N, R>,
  scope: Common.ToolMiddlewareScope,
): ImplementedMiddleware<N> => {
  if (entries.has(options.name))
    throw new Error(`Middleware '${options.name}' is already registered`)
  entries.set(options.name, {
    wire: {
      name: options.name,
      aliases: [...(options.aliases ?? [])],
      doc: normalizeDoc(options.doc),
      scope,
    },
    handler: options.handler,
    layer: options.layer,
  })
  return { name: options.name }
}

/** Define universal transparent middleware. @since 1.6.0 @category constructors */
export const universal = <N extends string, R = never>(options: MiddlewareOptions<N, R>) =>
  register(options, { tag: "universal" })

/** Define typed middleware projected from presented and expected tool definitions. @since 1.6.0 @category constructors */
export function typed<
  N extends string,
  P extends ToolDefinition<any, any>,
  D extends ToolDefinition<any, any>,
  R = never,
>(
  options: Omit<MiddlewareOptions<N, R>, "handler"> & {
    readonly presented: P
    readonly expected: D
    readonly handler: TypedImplementation<P, D, R>
  },
): ImplementedMiddleware<N>
export function typed<N extends string, P extends ToolDefinition<any, any>, R = never>(
  options: Omit<MiddlewareOptions<N, R>, "handler"> & {
    readonly presented: P
    readonly expected?: undefined
    readonly handler: TypedImplementation<P, P, R>
  },
): ImplementedMiddleware<N>
export function typed<N extends string, R = never>(
  options: Omit<MiddlewareOptions<N, R>, "handler"> & {
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
  ): UniversalHandler<any> =>
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
            val: { graph: declared.codec.schemaGraph, value },
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
      ? (input: Record<string, unknown>, streams?: TypedStreams) =>
          Effect.gen(function* () {
            if (!body.model.stdin && streams?.stdin)
              return yield* Effect.fail(
                new MiddlewareError({ tag: "invalid-input", val: "undeclared stdin stream" }),
              )
            if (body.model.stdin?.required && !streams?.stdin)
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
            const invocation = yield* raw
              .invoke(path, { graph: body.input.schemaGraph, value }, streams?.stdin)
              .pipe(
                Effect.catchTag("MiddlewareError", (error) => {
                  const custom = customToolError(error.cause)
                  if (!custom) return Effect.fail(error)
                  return Effect.gen(function* () {
                    let declared: (typeof body.errors)[number] | undefined
                    for (const entry of body.errors) {
                      const codec = yield* compile(entry.spec.schema)
                      if (sameWireGraph(codec.schemaGraph, custom.val.graph)) {
                        declared = entry
                        break
                      }
                    }
                    if (!declared) return yield* Effect.fail(error)
                    const decoded = yield* declared.codec.decode(custom.val.value)
                    return yield* Effect.fail(err(declared.spec.name, decoded))
                  }).pipe(
                    Effect.mapError((cause) =>
                      cause instanceof MiddlewareError || isFailure(cause)
                        ? cause
                        : new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
                    ),
                  )
                }),
              )
            if (isFailure(invocation)) return invocation
            const success = invocation as Common.InvocationResult
            if (body.model.stdout?.required && !success.stdout)
              return yield* Effect.fail(
                new MiddlewareError({
                  tag: "invalid-result",
                  val: "required stdout stream is missing",
                }),
              )
            if (success.stdout) {
              const stdout = inputStream(success.stdout)!
              yield* (streams?.stdout ? streams.stdout(stdout) : Stream.runDrain(stdout)).pipe(
                Effect.mapError((cause) =>
                  cause instanceof MiddlewareError
                    ? cause
                    : new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
                ),
              )
            }
            if (!body.output) return undefined
            if (!success.result)
              return yield* Effect.fail(
                new MiddlewareError({ tag: "invalid-result", val: "missing result" }),
              )
            return yield* body.output
              .decode(success.result.value)
              .pipe(
                Effect.mapError(
                  (cause) => new MiddlewareError({ tag: "invalid-result", val: String(cause) }),
                ),
              )
          })
      : {}
    for (const child of Object.values(model.children))
      node[camelCase(child.name)] = build(child, [...path, child.name])
    return node
  }
  return build(compiled.definition.model, [])
}

const customToolError = (
  cause: Common.ToolError,
): Extract<Common.ToolError, { tag: "custom-error" }> | undefined => {
  const tagged = cause as Common.ToolError | { tag: "remote-tool-error"; val: Common.ToolError }
  const error = tagged.tag === "remote-tool-error" ? tagged.val : tagged
  return error.tag === "custom-error" ? error : undefined
}

const sameWireGraph = (left: unknown, right: unknown): boolean =>
  JSON.stringify(left, (_key, value) => (typeof value === "bigint" ? `${value}n` : value)) ===
  JSON.stringify(right, (_key, value) => (typeof value === "bigint" ? `${value}n` : value))

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
    commandPath: string[],
    input: Common.TypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
    principal: Agent.Principal,
    wrapped: Common.UnderlyingTool,
  ): Promise<Common.InvocationResult> => {
    const entry = entries.get(middlewareName)
    if (!entry) throw { tag: "invalid-tool-name", val: middlewareName } satisfies Common.ToolError
    const ownership = new InvocationOwnership()
    const managedStdin = stdin ? ownership.own(stdin) : undefined
    const middlewareStdin = inputStream(managedStdin)
    const invocationScope = await Effect.runPromise(Scope.make())
    let active = true
    let busy = false
    const underlying: Underlying = {
      invoke: (path, value, source) =>
        Effect.gen(function* () {
          if (!active || busy) {
            return yield* Effect.fail(
              new MiddlewareError({
                tag: "invalid-input",
                val: "underlying tool used concurrently or after invocation",
              }),
            )
          }
          busy = true
          const iterable = source
            ? ownership.own(outputStreamWith(source, yield* Effect.context<never>()))
            : undefined
          const invocation = Promise.resolve()
            .then(() => wrapped.invoke([...path], value, iterable))
            .then((result) =>
              result.stdout ? { ...result, stdout: ownership.own(result.stdout) } : result,
            )
          void invocation.then(
            () => {
              busy = false
            },
            () => {
              busy = false
            },
          )
          return yield* Effect.tryPromise({
            try: () => invocation,
            catch: (cause) => new MiddlewareError(cause as Common.ToolError),
          })
        }),
    }
    const effect = Effect.suspend(() =>
      entry.handler(
        { toolName, tool, commandPath, input, stdin: middlewareStdin, principal },
        underlying,
      ),
    )
    const closeLayer = () => Effect.runPromise(Scope.close(invocationScope, Exit.void))
    const cleanup = async (primary?: unknown): Promise<void> => {
      const ownershipResult = await Promise.allSettled([ownership.dispose()])
      const layerResult = await Promise.allSettled([closeLayer()])
      if (primary !== undefined) throw primary
      const failure = [...ownershipResult, ...layerResult].find(
        (result): result is PromiseRejectedResult => result.status === "rejected",
      )
      if (failure) throw failure.reason
    }
    let context = Context.empty()
    try {
      if (entry.layer)
        context = await Effect.runPromise(Layer.buildWithScope(entry.layer, invocationScope))
      const result = await Effect.runPromise(
        Effect.provide(effect as Effect.Effect<Common.InvocationResult, MiddlewareError>, context),
      )
      active = false
      if (!result.stdout) {
        await cleanup()
        return result
      }
      const output = new FinalOutput(result.stdout, ownership.dispose.bind(ownership), closeLayer)
      return { ...result, stdout: output }
    } catch (cause) {
      active = false
      await cleanup(cause instanceof MiddlewareError ? cause.cause : cause)
      throw cause
    }
  },
}

class FinalOutput implements AsyncIterableIterator<number> {
  private readonly iterator: AsyncIterator<number>
  private closePromise: Promise<void> | undefined

  constructor(
    source: AsyncIterable<number>,
    private readonly unblock: () => Promise<void>,
    private readonly cleanup: () => Promise<void>,
  ) {
    this.iterator = source[Symbol.asyncIterator]()
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<number> {
    return this
  }

  async next(): Promise<IteratorResult<number>> {
    if (this.closePromise) return { done: true, value: undefined }
    try {
      const result = await this.iterator.next()
      if (result.done) await this.close()
      return result
    } catch (error) {
      await this.close().catch(() => undefined)
      throw error
    }
  }

  return(): Promise<IteratorResult<number>> {
    return this.close().then(() => ({ done: true, value: undefined }))
  }

  close(): Promise<void> {
    return (this.closePromise ??= (async () => {
      const returned = Promise.resolve().then(() => this.iterator.return?.())
      const results = await Promise.allSettled([this.unblock(), returned])
      const layer = await Promise.allSettled([this.cleanup()])
      const failure = [...results, ...layer].find(
        (result): result is PromiseRejectedResult => result.status === "rejected",
      )
      if (failure) throw failure.reason
    })())
  }
}
