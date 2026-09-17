/** Effect-native reflection for ambient Golem tools. @since 1.6.0 */
import type * as Common from "golem:tool/common@0.1.0"
import type * as Core from "golem:core/types@2.0.0"
import { Effect, Scope, Stream } from "effect"
import { createToolClientRuntime, type ToolRuntimeError } from "./BridgeTool.js"
import { ToolClient } from "./host/ToolClient.js"
import {
  field,
  schemaValueEquals,
  schemaShapesMatch,
  t,
  v,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
} from "./internal/schema-model/model.js"
import {
  schemaGraphRootsFromWit,
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
} from "./internal/schema-model/wit.js"
import { freezeSchemaGraph, SchemaRef, type JsonValue } from "./SchemaRef.js"

/** A local schema, metadata, or remote output failure. @since 1.6.0 @category errors */
export class ToolReflectionError {
  readonly _tag = "ToolReflectionError"
  constructor(
    readonly phase: "discovery" | "schema" | "input" | "invoke" | "output",
    readonly cause: unknown,
  ) {}
}

/** One declared argument in the effective command input. @since 1.6.0 @category models */
export interface ToolArgument {
  readonly name: string
  readonly aliases: ReadonlyArray<string>
  readonly kind: "positional" | "tail" | "option" | "flag"
  readonly required: boolean
  readonly schema: SchemaRef
  readonly optionalCarrier?: true
  readonly default?: SchemaValue
}

/** A declared custom tool failure decoded from the discovered schema. @since 1.6.0 @category errors */
export interface ReflectedToolFailure {
  readonly name: string
  readonly value?: JsonValue
}

/** A scoped command invocation with independent stdout and structured result. @since 1.6.0 @category streams */
export interface StartedToolInvocation<A, E = never> {
  readonly stdout: Stream.Stream<Uint8Array, ToolRuntimeError<never>>
  readonly result: Effect.Effect<A, ToolRuntimeError<E> | ToolReflectionError>
  readonly cancel: Effect.Effect<void>
  readonly collect: Effect.Effect<
    { readonly result: A; readonly stdout: Uint8Array },
    ToolRuntimeError<E> | ToolReflectionError
  >
}

/** A callable command in a discovered tool snapshot. @since 1.6.0 @category models */
export class ToolCommand {
  readonly name: string
  readonly path: ReadonlyArray<string>
  readonly aliases: ReadonlyArray<string>
  readonly doc: Common.Doc
  readonly children: ReadonlyArray<ToolCommand>
  readonly arguments: ReadonlyArray<ToolArgument>
  readonly constraints: ReadonlyArray<Common.Constraint>
  readonly inputSchema?: SchemaRef
  readonly stdin?: Common.StreamSpec
  readonly stdout?: Common.StreamSpec
  readonly result?: SchemaRef
  readonly errors: ReadonlyArray<{ readonly name: string; readonly payload?: SchemaRef }>
  private readonly wireGraph?: SchemaGraph

  constructor(
    readonly toolName: string,
    node: Common.CommandNode,
    path: ReadonlyArray<string>,
    inherited: ReadonlyArray<ToolArgument>,
    graph: SchemaGraph,
    typeAt: (index: number) => SchemaType,
    children: ReadonlyArray<ToolCommand>,
  ) {
    this.name = node.name
    this.path = Object.freeze([...path])
    this.aliases = Object.freeze([...node.aliases])
    this.doc = node.doc
    this.children = Object.freeze([...children])
    const globals = [
      ...node.globals.options.map((option) => optionArgument(option, graph, typeAt)),
      ...node.globals.flags.map((flag) => flagArgument(flag, graph)),
    ]
    const body = node.body
    const local: ToolArgument[] = body
      ? [
          ...body.positionals.fixed.map((positional) => {
            const optional = !positional.required && positional.default_ === undefined
            return Object.freeze({
              kind: "positional" as const,
              name: positional.name,
              aliases: [] as ReadonlyArray<string>,
              required: positional.required,
              optionalCarrier: optional ? (true as const) : undefined,
              default:
                positional.default_ === undefined
                  ? undefined
                  : schemaValueFromWit(positional.default_),
              schema: SchemaRef.fromImmutableGraph(
                graph,
                optional ? t.option(typeAt(positional.type)) : typeAt(positional.type),
              ),
            })
          }),
          ...(body.positionals.tail
            ? [
                Object.freeze({
                  kind: "tail" as const,
                  name: body.positionals.tail.name,
                  aliases: [] as ReadonlyArray<string>,
                  required: body.positionals.tail.min > 0,
                  schema: SchemaRef.fromImmutableGraph(
                    graph,
                    t.list(typeAt(body.positionals.tail.itemType)),
                  ),
                }),
              ]
            : []),
          ...body.options.map((option) => optionArgument(option, graph, typeAt)),
          ...body.flags.map((flag) => flagArgument(flag, graph)),
        ]
      : []
    const localNames = new Set(local.flatMap((argument) => [argument.name, ...argument.aliases]))
    this.arguments = Object.freeze(
      [...inherited, ...globals]
        .filter(
          (argument) => ![argument.name, ...argument.aliases].some((name) => localNames.has(name)),
        )
        .concat(local),
    )
    this.constraints = Object.freeze([...(body?.constraints ?? [])])
    this.stdin = body?.stdin
    this.stdout = body?.stdout
    this.result = body?.result
      ? SchemaRef.fromImmutableGraph(graph, typeAt(body.result.type))
      : undefined
    this.errors = Object.freeze(
      (body?.errors ?? []).map((error) =>
        Object.freeze({
          name: error.name,
          payload:
            error.payload === undefined
              ? undefined
              : SchemaRef.fromImmutableGraph(graph, typeAt(error.payload)),
        }),
      ),
    )
    if (body) {
      this.inputSchema = SchemaRef.fromImmutableGraph(
        graph,
        t.record(this.arguments.map((argument) => field(argument.name, argument.schema.root))),
      )
      this.wireGraph = Object.freeze({
        defs: graph.defs,
        root: t.record(
          this.arguments.map((argument) =>
            field(
              argument.name,
              argument.optionalCarrier && argument.schema.root.body.tag === "option"
                ? argument.schema.root.body.element
                : argument.schema.root,
            ),
          ),
        ),
      })
    }
    Object.freeze(this)
  }

  /** Pack canonical JSON before a remote call. @since 1.6.0 @category validation */
  packJson(input: JsonValue): Core.SchemaValueTree {
    if (!this.inputSchema) throw new ToolReflectionError("input", "command has no body")
    const value = this.inputSchema.packJson(input)
    this.validateConstraints(value)
    return value
  }

  /** Validate canonical JSON without dispatching. @since 1.6.0 @category validation */
  validateJson(input: JsonValue): ReturnType<SchemaRef["validateJson"]> {
    try {
      return { success: true, value: this.packJson(input) }
    } catch (error) {
      return { success: false, issues: [{ path: [], message: String(error) }] }
    }
  }

  private validateConstraints(value: Core.SchemaValueTree): void {
    const model = schemaValueFromWit(value)
    if (model.tag !== "record")
      throw new ToolReflectionError("input", "tool input must be a record")
    const values = new Map(
      this.arguments.map((argument, index) => [argument.name, model.fields[index]]),
    )
    const matches = (reference: Common.Ref): boolean => {
      const name = reference.tag === "present" ? reference.val : reference.val.name
      const argument = this.arguments.find(
        (candidate) => candidate.name === name || candidate.aliases.includes(name),
      )
      if (!argument)
        throw new ToolReflectionError("schema", `unknown constraint argument '${name}'`)
      const actual = values.get(argument.name)!
      return reference.tag === "present"
        ? valuePresent(actual, argument.default)
        : valueMatches(actual, schemaValueFromWit(reference.val.value))
    }
    const quantify = (refs: ReadonlyArray<Common.Ref>, quantifier: Common.Quantifier) =>
      quantifier === "all" ? refs.every(matches) : refs.some(matches)
    this.constraints.forEach((constraint, index) => {
      const satisfied = (() => {
        switch (constraint.tag) {
          case "requires-all":
            return quantify(constraint.val, "all")
          case "requires-any":
            return quantify(constraint.val, "any")
          case "all-or-none": {
            const count = constraint.val.filter(matches).length
            return count === 0 || count === constraint.val.length
          }
          case "mutex-groups":
            return constraint.val.filter((group) => quantify(group.refs, "all")).length <= 1
          case "implies":
            return (
              !quantify(constraint.val.lhs, constraint.val.lhsQuant) ||
              quantify(constraint.val.rhs, constraint.val.rhsQuant)
            )
          case "forbids":
            return (
              !quantify(constraint.val.lhs, constraint.val.lhsQuant) ||
              !quantify(constraint.val.rhs, "any")
            )
        }
      })()
      if (!satisfied)
        throw new ToolReflectionError("input", `tool command constraint ${index} is not satisfied`)
    })
  }

  /** Start a scoped schema-native call. @since 1.6.0 @category invocation */
  startValue(
    input: Core.SchemaValueTree,
    stdin?: Stream.Stream<Uint8Array>,
  ): Effect.Effect<
    StartedToolInvocation<Core.SchemaValueTree | undefined, ReflectedToolFailure>,
    ToolReflectionError | ToolRuntimeError<ReflectedToolFailure>,
    ToolClient | Scope.Scope
  > {
    const command = {
      inputSchema: this.inputSchema,
      wireGraph: this.wireGraph,
      stdin: this.stdin,
      stdout: this.stdout,
      result: this.result,
      toolName: this.toolName,
      path: this.path,
      validateConstraints: (value: Core.SchemaValueTree) => this.validateConstraints(value),
      mapFailure: (error: ToolRuntimeError<never>) => this.mapFailure(error),
    }
    return Effect.gen(function* () {
      if (!command.inputSchema || !command.wireGraph)
        return yield* Effect.fail(new ToolReflectionError("input", "command has no body"))
      if (!command.inputSchema.validateValue(input).success)
        return yield* Effect.fail(new ToolReflectionError("input", "invalid tool input"))
      yield* Effect.try({
        try: () => command.validateConstraints(input),
        catch: (cause) => new ToolReflectionError("input", cause),
      })
      if (command.stdin?.required && !stdin)
        return yield* Effect.fail(new ToolReflectionError("input", "command requires stdin"))
      const runtime = createToolClientRuntime(command.toolName)
      const started = yield* runtime.start<never>(
        command.path,
        { graph: command.wireGraph, value: schemaValueFromWit(input) },
        stdin,
        command.stdout !== undefined,
      )
      const stdout = started.stdout ?? Stream.empty
      const result = started.result.pipe(
        Effect.mapError((error) => command.mapFailure(error)),
        Effect.flatMap((terminal) => {
          if (!command.result) {
            return terminal.result === undefined
              ? Effect.succeed(undefined)
              : Effect.fail(new ToolReflectionError("output", "unexpected remote result"))
          }
          if (!terminal.result)
            return Effect.fail(
              new ToolReflectionError("output", "missing or malformed remote result"),
            )
          return Effect.try({
            try: () =>
              schemaShapesMatch(
                command.result!.graph,
                schemaGraphFromWit(terminal.result!.graph),
              ) && command.result!.validateValue(terminal.result!.value).success,
            catch: (cause) => new ToolReflectionError("output", cause),
          }).pipe(
            Effect.flatMap((valid) =>
              valid
                ? Effect.succeed(terminal.result!.value)
                : Effect.fail(new ToolReflectionError("output", "malformed remote result")),
            ),
          )
        }),
      )
      const collect = Effect.all([result, Stream.runCollect(stdout)], {
        concurrency: "unbounded",
      }).pipe(
        Effect.map(([value, chunks]) => ({
          result: value,
          stdout: concatBytes(chunks),
        })),
      )
      return { stdout, result, cancel: started.cancel, collect }
    })
  }

  /** Start a scoped canonical JSON call. @since 1.6.0 @category invocation */
  startJson(
    input: JsonValue,
    stdin?: Stream.Stream<Uint8Array>,
  ): Effect.Effect<
    StartedToolInvocation<JsonValue | undefined, ReflectedToolFailure>,
    ToolReflectionError | ToolRuntimeError<ReflectedToolFailure>,
    ToolClient | Scope.Scope
  > {
    return Effect.try({
      try: () => this.packJson(input),
      catch: (cause) => new ToolReflectionError("input", cause),
    }).pipe(
      Effect.flatMap((value) => this.startValue(value, stdin)),
      Effect.map((started) => {
        const result = started.result.pipe(
          Effect.flatMap((value) =>
            value === undefined
              ? Effect.succeed(undefined)
              : Effect.try({
                  try: () => this.result!.unpackJson(value),
                  catch: (cause) => new ToolReflectionError("output", cause),
                }),
          ),
        )
        const collect = Effect.all([result, Stream.runCollect(started.stdout)], {
          concurrency: "unbounded",
        }).pipe(Effect.map(([value, chunks]) => ({ result: value, stdout: concatBytes(chunks) })))
        return { stdout: started.stdout, result, cancel: started.cancel, collect }
      }),
    )
  }

  /** Await a schema-native result, draining optional stdout concurrently. @since 1.6.0 @category invocation */
  invokeValue(input: Core.SchemaValueTree, stdin?: Stream.Stream<Uint8Array>) {
    if (this.stdout?.required)
      return Effect.fail(
        new ToolReflectionError("input", "command requires caller-readable stdout"),
      )
    return Effect.scoped(
      this.startValue(input, stdin).pipe(
        Effect.flatMap((started) => started.collect),
        Effect.map((v) => v.result),
      ),
    )
  }

  /** Await a canonical JSON result. @since 1.6.0 @category invocation */
  invokeJson(input: JsonValue, stdin?: Stream.Stream<Uint8Array>) {
    return Effect.try({
      try: () => this.packJson(input),
      catch: (cause) => new ToolReflectionError("input", cause),
    }).pipe(
      Effect.flatMap((value) => this.invokeValue(value, stdin)),
      Effect.flatMap((value) =>
        value === undefined
          ? Effect.succeed(undefined)
          : Effect.try({
              try: () => this.result!.unpackJson(value),
              catch: (cause) => new ToolReflectionError("output", cause),
            }),
      ),
    )
  }

  /** Admit work without a result or stdout observer. @since 1.6.0 @category invocation */
  triggerValue(input: Core.SchemaValueTree, stdin?: Stream.Stream<Uint8Array>) {
    const command = {
      inputSchema: this.inputSchema,
      wireGraph: this.wireGraph,
      stdin: this.stdin,
      stdout: this.stdout,
      toolName: this.toolName,
      path: this.path,
      validateConstraints: (value: Core.SchemaValueTree) => this.validateConstraints(value),
    }
    return Effect.gen(function* () {
      if (command.stdout?.required)
        return yield* Effect.fail(
          new ToolReflectionError("input", "command requires caller-readable stdout"),
        )
      if (command.stdin?.required && !stdin)
        return yield* Effect.fail(new ToolReflectionError("input", "command requires stdin"))
      if (
        !command.inputSchema ||
        !command.wireGraph ||
        !command.inputSchema.validateValue(input).success
      )
        return yield* Effect.fail(new ToolReflectionError("input", "invalid tool input"))
      yield* Effect.try({
        try: () => command.validateConstraints(input),
        catch: (cause) => new ToolReflectionError("input", cause),
      })
      const host = yield* ToolClient
      const rpc = yield* Effect.try({
        try: () => host.rpc(command.toolName),
        catch: (cause) => new ToolReflectionError("invoke", cause),
      })
      const source = stdin
        ? yield* Stream.toAsyncIterableEffect(
            Stream.mapEffect(stdin, (value) =>
              value.byteLength > 0
                ? Effect.succeed({ tag: "ok" as const, val: value })
                : Effect.fail(new ToolReflectionError("input", "stdin yielded an empty chunk")),
            ),
          )
        : undefined
      yield* Effect.try({
        try: () =>
          rpc.invoke(
            [...command.path],
            {
              graph: schemaGraphToWit(command.wireGraph!),
              value: input,
            },
            source ? host.createStdinFromStream(source) : undefined,
          ),
        catch: (cause) => new ToolReflectionError("invoke", cause),
      })
    })
  }

  /** Admit canonical JSON work without observing its result. @since 1.6.0 @category invocation */
  triggerJson(input: JsonValue, stdin?: Stream.Stream<Uint8Array>) {
    return Effect.try({
      try: () => this.packJson(input),
      catch: (cause) => new ToolReflectionError("input", cause),
    }).pipe(Effect.flatMap((value) => this.triggerValue(value, stdin)))
  }

  private mapFailure(
    error: ToolRuntimeError<never>,
  ): ToolRuntimeError<ReflectedToolFailure> | ToolReflectionError {
    if (
      error.tag !== "rpc" ||
      error.error.tag !== "remote-tool-error" ||
      error.error.val.tag !== "custom-error"
    )
      return error
    const custom = error.error.val.val
    const declared = this.errors.find((entry) => entry.name === custom.name)
    if (!declared) return error
    try {
      if (!declared.payload) {
        const value = schemaValueFromWit(custom.payload.value)
        if (value.tag !== "tuple" || value.elements.length !== 0)
          return new ToolReflectionError("output", "custom error has an unexpected payload")
        return { tag: "tool", error: { name: custom.name } }
      }
      if (
        !schemaShapesMatch(declared.payload.graph, schemaGraphFromWit(custom.payload.graph)) ||
        !declared.payload.validateValue(custom.payload.value).success
      )
        return new ToolReflectionError("output", "custom error payload is malformed")
      return {
        tag: "tool",
        error: {
          name: custom.name,
          value: declared.payload.unpackJson(custom.payload.value),
        },
      }
    } catch (cause) {
      return new ToolReflectionError("output", cause)
    }
  }
}

/** Strict callable command lookup for one reflected tool. @since 1.6.0 @category models */
export interface ReflectedToolClient {
  readonly command: (path: ReadonlyArray<string>) => ToolCommand
}

/** An immutable runtime-discovered tool registration. @since 1.6.0 @category models */
export class ToolType {
  readonly name: string
  readonly lookupName: string
  readonly version: string
  readonly implementedBy: Core.ComponentId
  readonly commands: ReadonlyArray<ToolCommand>
  readonly client: ReflectedToolClient

  constructor(registered: import("golem:tool/host@0.1.0").RegisteredTool) {
    const raw = registered.definition
    if (raw.commands.nodes.length === 0) throw new TypeError("tool has no root command")
    this.name = raw.commands.nodes[0].name
    this.lookupName = registered.lookupName
    this.version = raw.version
    this.implementedBy = registered.implementedBy
    const decoded = schemaGraphRootsFromWit(
      raw.schema,
      raw.schema.typeNodes.map((_, i) => i),
    )
    const graph = freezeSchemaGraph({ defs: decoded.defs, root: decoded.roots[raw.schema.root] })
    const typeAt = (index: number) => {
      const selected = decoded.roots[index]
      if (!selected) throw new TypeError(`tool schema node ${index} is missing`)
      return selected
    }
    const seen = new Set<number>()
    const build = (
      index: number,
      path: ReadonlyArray<string>,
      inherited: ReadonlyArray<ToolArgument>,
    ): ToolCommand => {
      if (seen.has(index)) throw new TypeError("tool command tree has a cycle or shared child")
      const node = raw.commands.nodes[index]
      if (!node) throw new TypeError(`tool command node ${index} is missing`)
      seen.add(index)
      const globals = [
        ...node.globals.options.map((option) => optionArgument(option, graph, typeAt)),
        ...node.globals.flags.map((flag) => flagArgument(flag, graph)),
      ]
      const children = node.subcommands.map((child) =>
        build(child, [...path, raw.commands.nodes[child]?.name ?? ""], [...inherited, ...globals]),
      )
      return new ToolCommand(this.lookupName, node, path, inherited, graph, typeAt, children)
    }
    this.commands = Object.freeze([build(0, [], [])])
    if (seen.size !== raw.commands.nodes.length)
      throw new TypeError("tool has unreachable commands")
    this.client = Object.freeze({
      command: (path: ReadonlyArray<string>) => {
        const command = this.command(path)
        if (!command?.inputSchema)
          throw new TypeError(`unknown callable command '${path.join(" ")}'`)
        return command
      },
    })
    Object.freeze(this)
  }

  /** Resolve names and aliases to a canonical command. @since 1.6.0 @category lookup */
  command(path: ReadonlyArray<string>): ToolCommand | undefined {
    let current = this.commands[0]
    for (const segment of path) {
      current = current.children.find(
        (child) => child.name === segment || child.aliases.includes(segment),
      )!
      if (!current) return undefined
    }
    return current
  }
}

/** A schema-free client bound only to an ambient tool name. @since 1.6.0 @category models */
export class DynamicToolClient {
  constructor(readonly name: string) {}

  /** Start a scoped call with a caller-owned packed value. @since 1.6.0 @category invocation */
  start(
    path: ReadonlyArray<string>,
    input: Core.TypedSchemaValue,
    stdin?: Stream.Stream<Uint8Array>,
    withStdout = false,
  ): Effect.Effect<
    StartedToolInvocation<Core.TypedSchemaValue | undefined>,
    ToolReflectionError | ToolRuntimeError<never>,
    ToolClient | Scope.Scope
  > {
    const name = this.name
    return Effect.gen(function* () {
      const encoded = yield* Effect.try({
        try: () => ({
          graph: schemaGraphFromWit(input.graph),
          value: schemaValueFromWit(input.value),
        }),
        catch: (cause) => new ToolReflectionError("input", cause),
      })
      const started = yield* createToolClientRuntime(name).start<never>(
        path,
        encoded,
        stdin,
        withStdout,
      )
      const stdout = started.stdout ?? Stream.empty
      const result = started.result.pipe(Effect.map((terminal) => terminal.result))
      const collect = Effect.all([result, Stream.runCollect(stdout)], {
        concurrency: "unbounded",
      }).pipe(Effect.map(([value, chunks]) => ({ result: value, stdout: concatBytes(chunks) })))
      return { stdout, result, cancel: started.cancel, collect }
    })
  }

  /** Await a packed result and drain stdout. @since 1.6.0 @category invocation */
  invoke(
    path: ReadonlyArray<string>,
    input: Core.TypedSchemaValue,
    stdin?: Stream.Stream<Uint8Array>,
  ) {
    return Effect.scoped(
      this.start(path, input, stdin).pipe(
        Effect.flatMap((started) => started.collect),
        Effect.map((collected) => collected.result),
      ),
    )
  }

  /** Admit a caller-owned packed value without observing its result. @since 1.6.0 @category invocation */
  trigger(
    path: ReadonlyArray<string>,
    input: Core.TypedSchemaValue,
    stdin?: Stream.Stream<Uint8Array>,
  ) {
    const name = this.name
    return Effect.gen(function* () {
      const host = yield* ToolClient
      const rpc = yield* Effect.try({
        try: () => host.rpc(name),
        catch: (cause) => new ToolReflectionError("invoke", cause),
      })
      const source = stdin
        ? yield* Stream.toAsyncIterableEffect(
            Stream.map(stdin, (value) => ({ tag: "ok" as const, val: value })),
          )
        : undefined
      yield* Effect.try({
        try: () =>
          rpc.invoke([...path], input, source ? host.createStdinFromStream(source) : undefined),
        catch: (cause) => new ToolReflectionError("invoke", cause),
      })
    })
  }
}

/** Enumerate accessible native tool types. @since 1.6.0 @category discovery */
export const getAllToolTypes = (): Effect.Effect<
  ReadonlyArray<ToolType>,
  ToolReflectionError,
  ToolClient
> =>
  Effect.gen(function* () {
    const host = yield* ToolClient
    return yield* Effect.try({
      try: () => host.getAllTools().map((registered) => new ToolType(registered)),
      catch: (cause) => new ToolReflectionError("discovery", cause),
    })
  })

/** Look up one accessible native tool type. @since 1.6.0 @category discovery */
export const getToolType = (
  name: string,
): Effect.Effect<ToolType | undefined, ToolReflectionError, ToolClient> =>
  Effect.gen(function* () {
    const host = yield* ToolClient
    return yield* Effect.try({
      try: () => {
        const registered = host.getTool(name)
        return registered ? new ToolType(registered) : undefined
      },
      catch: (cause) => new ToolReflectionError("discovery", cause),
    })
  })

function optionArgument(
  option: Common.OptionSpec,
  graph: SchemaGraph,
  typeAt: (index: number) => SchemaType,
): ToolArgument {
  const root =
    option.shape.tag === "repeatable-list"
      ? t.list(typeAt(option.shape.val.itemType))
      : option.shape.tag === "repeatable-map"
        ? typeAt(option.shape.val.mapType)
        : typeAt(option.shape.val)
  const optional =
    !option.required &&
    option.default_ === undefined &&
    option.shape.tag !== "repeatable-list" &&
    option.shape.tag !== "repeatable-map"
  return Object.freeze({
    kind: "option",
    name: option.long,
    aliases: Object.freeze([...option.aliases]),
    required: option.required,
    optionalCarrier: optional ? true : undefined,
    default: option.default_ === undefined ? undefined : schemaValueFromWit(option.default_),
    schema: SchemaRef.fromImmutableGraph(graph, optional ? t.option(root) : root),
  })
}

function flagArgument(flag: Common.FlagSpec, graph: SchemaGraph): ToolArgument {
  return Object.freeze({
    kind: "flag",
    name: flag.long,
    aliases: Object.freeze([...flag.aliases]),
    required: false,
    default: flag.shape.tag === "bool-flag" ? v.bool(flag.shape.val.default_) : v.u32(0),
    schema: SchemaRef.fromImmutableGraph(
      graph,
      flag.shape.tag === "bool-flag" ? t.bool() : t.u32(),
    ),
  })
}

function valuePresent(value: SchemaValue, defaultValue?: SchemaValue): boolean {
  if (defaultValue !== undefined && schemaValueEquals(value, defaultValue)) return false
  switch (value.tag) {
    case "option":
      return value.value !== undefined
    case "list":
    case "fixed-list":
      return value.elements.length > 0
    case "map":
      return value.entries.length > 0
    case "bool":
      return value.value
    case "u32":
      return value.value !== 0
    default:
      return true
  }
}

function valueMatches(value: SchemaValue, expected: SchemaValue): boolean {
  if (schemaValueEquals(value, expected)) return true
  switch (value.tag) {
    case "option":
      return value.value !== undefined && valueMatches(value.value, expected)
    case "list":
    case "fixed-list":
      return value.elements.some((item) => valueMatches(item, expected))
    case "map":
      return value.entries.some((entry) => valueMatches(entry.value, expected))
    default:
      return false
  }
}

function concatBytes(chunks: ReadonlyArray<Uint8Array>): Uint8Array {
  const result = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0))
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }
  return result
}
