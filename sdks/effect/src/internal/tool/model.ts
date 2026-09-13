import type * as ToolCommon from "golem:tool/common@0.1.0"
import type * as Core from "golem:core/types@2.0.0"
import { Effect, Layer, Schema, Stream } from "effect"
import { GraphEncoder } from "../schema-model/wit.js"
import { composeSchemaGraphs } from "../schema-model/builder.js"
import { compile, type CompiledWitCodec } from "../../WitCodec.js"
import { Uint32 } from "../../WitTypes.js"
import type { HostServices } from "../../host/HostLive.js"
import type { Scope } from "effect"

export type DocInput = string | Partial<ToolCommon.Doc>
export type RepeatableMode =
  | "repeated"
  | "delimited"
  | "either"
  | { readonly delimited: string }
  | { readonly either: string }
export type CommandAnnotations = Partial<ToolCommon.CommandAnnotations>
export type Fields = Readonly<Record<string, Schema.Top>>
export type Decoded<F extends Fields> = { readonly [K in keyof F]: F[K]["Type"] }
export type CommandInput<B extends BodyModel> =
  B extends BodyModel<infer F, any, any> ? Decoded<F> : never
export type CommandOutput<B extends BodyModel> =
  B extends BodyModel<any, infer O, any> ? (O extends Schema.Top ? O["Type"] : void) : never
export type CommandError<B extends BodyModel> =
  B extends BodyModel<any, any, infer Errors>
    ? Errors[number] extends infer E
      ? E extends ToolErrorCase<infer N, infer S>
        ? ToolFailure<N, S["Type"]>
        : never
      : never
    : never
export type Services<F extends Fields> =
  | F[keyof F]["DecodingServices"]
  | F[keyof F]["EncodingServices"]

export type StreamOptions = Partial<ToolCommon.StreamSpec>
export interface ArgumentOptions {
  readonly aliases?: readonly string[]
  readonly short?: string
  readonly doc?: DocInput
  readonly valueName?: string
  readonly required?: boolean
  readonly default?: unknown
  readonly envVar?: string
  readonly env?: string
  readonly acceptsStdio?: boolean
  readonly optionalScalar?: boolean
  readonly delim?: string
  readonly duplicateKeyPolicy?: ToolCommon.DuplicateKeyPolicy
}
export interface ErrorOptions {
  readonly kind?: ToolCommon.ErrorKind
  readonly exitCode?: number
  readonly doc?: DocInput
}
export interface ResultOptions {
  readonly doc?: DocInput
  readonly formatters?: readonly (string | ToolCommon.Formatter)[]
  readonly defaultFormatter?: string
}

export interface ToolErrorCase<Name extends string = string, S extends Schema.Top = Schema.Top> {
  readonly name: Name
  readonly schema: S
  readonly options: ErrorOptions
}
export interface ToolSuccess<A> {
  readonly _tag: "ToolSuccess"
  readonly value: A
}
export interface ToolFailure<N extends string, A> {
  readonly _tag: "ToolFailure"
  readonly name: N
  readonly value: A
}
export const ok = <A>(value: A): ToolSuccess<A> => ({ _tag: "ToolSuccess", value })
export const err = <N extends string, A>(name: N, value: A): ToolFailure<N, A> => ({
  _tag: "ToolFailure",
  name,
  value,
})

export class ToolInvokeError extends Error {
  readonly _tag = "ToolInvokeError"
  constructor(readonly cause: ToolCommon.ToolError) {
    super(`Tool invocation failed: ${cause.tag}`)
  }
}

interface ArgumentSpec {
  readonly kind: "positional" | "option" | "flag" | "tail"
  readonly name: string
  readonly schema: Schema.Top
  readonly wireSchema: Schema.Top
  readonly options: ArgumentOptions
  readonly flag?: "boolean" | "count"
  readonly repeatable?: RepeatableMode
  readonly global?: boolean
}
export interface BodyModel<
  F extends Fields = Fields,
  O extends Schema.Top | undefined = Schema.Top | undefined,
  Errors extends readonly ToolErrorCase[] = readonly ToolErrorCase[],
> {
  readonly fields: F
  readonly args: readonly ArgumentSpec[]
  readonly output?: O
  readonly resultOptions?: ResultOptions
  readonly errors: Errors
  readonly constraints: readonly ToolCommon.Constraint[]
  readonly stdin?: StreamOptions
  readonly stdout?: StreamOptions
  readonly annotations?: CommandAnnotations
}

export class BodyBuilder<
  F extends Fields = Record<never, never>,
  O extends Schema.Top | undefined = undefined,
  Errors extends readonly ToolErrorCase[] = [],
> {
  constructor(
    readonly model: BodyModel<F, O, Errors> = {
      fields: {} as F,
      args: [],
      errors: [] as unknown as Errors,
      constraints: [],
    },
  ) {}
  private add<N extends string, S extends Schema.Top, FS extends Schema.Top = S>(
    kind: ArgumentSpec["kind"],
    name: N,
    schema: S,
    options: ArgumentOptions = {},
    extra: Partial<ArgumentSpec> = {},
    fieldSchema: FS = schema as unknown as FS,
  ): BodyBuilder<F & Record<N, FS>, O, Errors> {
    return new BodyBuilder({
      ...this.model,
      fields: { ...this.model.fields, [name]: fieldSchema },
      args: [
        ...this.model.args,
        { kind, name, schema: fieldSchema, wireSchema: schema, options, ...extra },
      ],
    } as never)
  }
  positional<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options?: ArgumentOptions,
  ) {
    return this.add("positional", name, schema, options)
  }
  option<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options: ArgumentOptions & { readonly repeatable: RepeatableMode },
  ): BodyBuilder<F & Record<N, ReturnType<typeof Schema.Array<S>>>, O, Errors>
  option<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options?: ArgumentOptions & { readonly repeatable?: undefined },
  ): BodyBuilder<F & Record<N, S>, O, Errors>
  option<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options?: ArgumentOptions & { readonly repeatable?: RepeatableMode },
  ) {
    const repeatable = options?.repeatable
    const compiled = Effect.runSync(compile(schema))
    const root = resolveRoot(compiled)
    const repeatedSchema = repeatable && root.tag !== "map" ? Schema.Array(schema) : schema
    return this.add("option", name, schema, options, { repeatable }, repeatedSchema)
  }
  flag<N extends string>(name: N, options?: ArgumentOptions & { readonly negatable?: boolean }) {
    return this.add("flag", name, Schema.Boolean, options, { flag: "boolean" })
  }
  countFlag<N extends string>(name: N, options?: ArgumentOptions & { readonly max?: number }) {
    return this.add("flag", name, Uint32, options, { flag: "count" })
  }
  tail<N extends string, S extends Schema.Top>(
    name: N,
    item: S,
    options?: ArgumentOptions & {
      readonly min?: number
      readonly max?: number
      readonly separator?: string
      readonly verbatim?: boolean
    },
  ) {
    return this.add("tail", name, item, options, {}, Schema.Array(item))
  }
  globalOption<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options?: ArgumentOptions,
  ) {
    return this.add("option", name, schema, options, { global: true })
  }
  globalFlag<N extends string>(name: N, options?: ArgumentOptions) {
    return this.add("flag", name, Schema.Boolean, options, { global: true, flag: "boolean" })
  }
  input(options: StreamOptions = {}): BodyBuilder<F, O, Errors> {
    return new BodyBuilder({ ...this.model, stdin: options })
  }
  output(options: StreamOptions = {}): BodyBuilder<F, O, Errors> {
    return new BodyBuilder({ ...this.model, stdout: options })
  }
  returns<S extends Schema.Top>(schema: S, options?: ResultOptions): BodyBuilder<F, S, Errors> {
    const formatters = options?.formatters ?? ["default"]
    const first = formatters[0]
    return new BodyBuilder({
      ...this.model,
      output: schema,
      resultOptions: {
        ...options,
        formatters,
        defaultFormatter:
          options?.defaultFormatter ??
          (typeof first === "string" ? first : first?.name) ??
          "default",
      },
    } as never)
  }
  error<N extends string, S extends Schema.Top>(
    name: N,
    schema: S,
    options: ErrorOptions = {},
  ): BodyBuilder<F, O, [...Errors, ToolErrorCase<N, S>]> {
    return new BodyBuilder({
      ...this.model,
      errors: [...this.model.errors, { name, schema, options }],
    } as never)
  }
  constraint(value: ToolCommon.Constraint) {
    return new BodyBuilder({ ...this.model, constraints: [...this.model.constraints, value] })
  }
  annotate(value: CommandAnnotations) {
    return new BodyBuilder({ ...this.model, annotations: value })
  }
}

export type HandlerContext = {
  readonly principal: unknown
  readonly stdin?: Stream.Stream<Uint8Array, ToolInvokeError>
  readonly stdout?: <R2>(
    source: Stream.Stream<Uint8Array, ToolInvokeError, R2>,
  ) => Effect.Effect<void, ToolInvokeError, R2>
}
export type Handler<B extends BodyModel, R = never> = (
  input: Decoded<B["fields"]>,
  context: HandlerContext,
) => Effect.Effect<
  CommandOutput<B> | ToolSuccess<CommandOutput<B>> | CommandError<B>,
  ToolInvokeError | CommandError<B>,
  R
>

type CamelCase<S extends string> = S extends `${infer H}-${infer T}`
  ? `${H}${Capitalize<CamelCase<T>>}`
  : S
type BodyServices<B extends BodyModel> =
  | Services<B["fields"]>
  | (B["output"] extends Schema.Top ? B["output"]["EncodingServices"] : never)
  | (B["errors"][number] extends infer E
      ? E extends ToolErrorCase<any, infer S>
        ? S["EncodingServices"]
        : never
      : never)
type ImplementationNode<M extends CommandModel, R> = M extends {
  readonly body: infer B extends BodyModel
}
  ? Handler<B, R | BodyServices<B>>
  : {
      readonly [K in keyof M["children"] as CamelCase<string & K>]: ImplementationNode<
        M["children"][K],
        R
      >
    }
type RuntimeProvided = HostServices | Scope.Scope
type ImplementationRequirements<M extends CommandModel, R> = Exclude<
  R | ModelServices<M>,
  RuntimeProvided
>
type ModelServices<M extends CommandModel> =
  | (M extends { readonly body: infer B extends BodyModel } ? BodyServices<B> : never)
  | (string extends keyof M["children"]
      ? never
      : { [K in keyof M["children"]]: ModelServices<M["children"][K]> }[keyof M["children"]])
export interface CommandModel {
  readonly name: string
  readonly aliases: readonly string[]
  readonly doc?: DocInput
  readonly body?: BodyModel
  readonly children: Readonly<Record<string, CommandModel>>
}
export class CommandBuilder<M extends CommandModel = CommandModel> {
  constructor(readonly model: M) {}
  declare readonly name: M["name"]
  implement(
    implementation: [ImplementationRequirements<M, never>] extends [never]
      ? ToolImplementation<M>
      : never,
  ): ImplementedTool<M["name"]>
  implement<R>(
    implementation: ToolImplementation<M, R>,
    layer: Layer.Layer<ImplementationRequirements<M, R>>,
  ): ImplementedTool<M["name"]>
  implement<R>(
    implementation: ToolImplementation<M, R>,
    layer?: Layer.Layer<ImplementationRequirements<M, R>>,
  ): ImplementedTool<M["name"]> {
    return registerTool(
      this as unknown as ToolDefinition<M["name"]>,
      implementation,
      layer as Layer.Layer<any> | undefined,
    )
  }
  body<B extends BodyBuilder<any, any, any>>(
    build: (body: BodyBuilder) => B,
  ): CommandBuilder<Omit<M, "body"> & { readonly body: B["model"] }> {
    return new CommandBuilder({ ...this.model, body: build(new BodyBuilder()).model } as never)
  }
  command<N extends string, C extends CommandBuilder<any>>(
    name: N,
    build: (command: CommandBuilder<{ name: N; aliases: []; children: Record<never, never> }>) => C,
    options: { aliases?: readonly string[]; doc?: DocInput } = {},
  ): CommandBuilder<
    Omit<M, "children"> & { readonly children: M["children"] & Record<N, C["model"]> }
  > {
    const child = build(new CommandBuilder({ name, aliases: [], children: {} }))
    return new CommandBuilder({
      ...this.model,
      children: { ...this.model.children, [name]: { ...child.model, ...options } },
    }) as never
  }
}

export interface ToolDefinition<
  Name extends string = string,
  M extends CommandModel = CommandModel,
> {
  readonly name: Name
  readonly model: M
}
export interface ErasedToolImplementation {
  readonly [name: string]: Handler<any, any> | ErasedToolImplementation
}
export type ToolImplementation<M extends CommandModel = never, R = never> = [M] extends [never]
  ? ErasedToolImplementation
  : (M extends { readonly body: infer B extends BodyModel }
      ? { readonly [K in M["name"] as CamelCase<string & K>]: Handler<B, R | BodyServices<B>> }
      : object) & {
      readonly [K in keyof M["children"] as CamelCase<string & K>]: ImplementationNode<
        M["children"][K],
        R
      >
    }
export interface ImplementedTool<Name extends string = string> {
  readonly name: Name
  readonly definition: ToolDefinition<Name>
}
export const toolDefinition = <N extends string>(
  name: N,
  options: { aliases?: readonly string[]; doc?: DocInput } = {},
): CommandBuilder<{ name: N; aliases: readonly string[]; children: Record<never, never> }> => {
  let model: CommandModel = {
    name,
    aliases: options.aliases ?? [],
    doc: options.doc,
    children: {},
  }
  const definition = Object.assign(Object.create(CommandBuilder.prototype), {
    name,
    body: (build: (body: BodyBuilder) => BodyBuilder<any, any, any>) => {
      model = { ...model, body: build(new BodyBuilder()).model }
      return definition
    },
    command: (
      childName: string,
      build: (command: CommandBuilder<any>) => CommandBuilder<any>,
      childOptions: { aliases?: readonly string[]; doc?: DocInput } = {},
    ) => {
      const child = build(new CommandBuilder({ name: childName, aliases: [], children: {} })).model
      model = {
        ...model,
        children: {
          ...model.children,
          [childName]: { ...child, ...childOptions },
        },
      }
      return definition
    },
    implement: (implementation: ErasedToolImplementation, layer?: Layer.Layer<any>) =>
      registerTool(definition, implementation, layer),
  }) as CommandBuilder<{ name: N; aliases: readonly string[]; children: Record<never, never> }>
  Object.defineProperty(definition, "model", { get: () => model })
  return definition
}

export interface CompiledBody {
  readonly model: BodyModel
  readonly args: readonly ArgumentSpec[]
  readonly input: CompiledWitCodec<any>
  readonly output?: CompiledWitCodec<any>
  readonly errors: readonly { spec: ToolErrorCase; codec: CompiledWitCodec<any> }[]
}
export interface Registered {
  readonly definition: ToolDefinition
  readonly wire: ToolCommon.Tool
  readonly bodies: ReadonlyMap<string, CompiledBody>
  readonly implementation: ToolImplementation
  readonly layer?: Layer.Layer<any>
}
const registry = new Map<string, Registered>()

const doc = (input?: DocInput): ToolCommon.Doc =>
  typeof input === "string"
    ? { summary: input, description: "", examples: [] }
    : {
        summary: input?.summary ?? "",
        description: input?.description ?? "",
        examples: input?.examples ?? [],
      }
const rep = (r: RepeatableMode): ToolCommon.Repetition =>
  r === "repeated"
    ? { tag: "repeated" }
    : r === "delimited" || r === "either"
      ? { tag: r, val: "," }
      : "delimited" in r
        ? { tag: "delimited", val: r.delimited }
        : { tag: "either", val: r.either }

export function compileDefinition(
  definition: ToolDefinition,
): Omit<Registered, "implementation" | "layer"> {
  validateDefinition(definition.model)
  const bodies = new Map<string, CompiledBody>()
  const nodes: ToolCommon.CommandNode[] = []
  const codecs: CompiledWitCodec<any>[] = []
  const inputCodecs = new Map<string, CompiledWitCodec<any>>()
  const compiledSchemas = new Map<Schema.Top, CompiledWitCodec<any>>()
  const compileOnce = (schema: Schema.Top) => {
    let codec = compiledSchemas.get(schema)
    if (!codec) {
      codec = Effect.runSync(compile(schema))
      compiledSchemas.set(schema, codec)
      codecs.push(codec)
    }
    return codec
  }
  const ordered = (args: readonly ArgumentSpec[]) => [
    ...args.filter((a) => a.kind === "option" && a.global),
    ...args.filter((a) => a.kind === "flag" && a.global),
    ...args.filter((a) => a.kind === "positional"),
    ...args.filter((a) => a.kind === "tail"),
    ...args.filter((a) => a.kind === "option" && !a.global),
    ...args.filter((a) => a.kind === "flag" && !a.global),
  ]
  const collect = (
    m: CommandModel,
    path: readonly string[],
    inherited: readonly ArgumentSpec[],
  ): void => {
    const localGlobals = m.body?.args.filter((a) => a.global) ?? []
    if (m.body) {
      const args = ordered([...inherited, ...m.body.args])
      inputCodecs.set(
        path.join("/"),
        compileOnce(Schema.Struct(Object.fromEntries(args.map((a) => [a.name, a.schema])))),
      )
      for (const argument of args) compileOnce(argument.wireSchema)
      if (m.body.output) compileOnce(m.body.output)
      for (const error of m.body.errors) compileOnce(error.schema)
    }
    for (const child of Object.values(m.children))
      collect(child, [...path, child.name], [...inherited, ...localGlobals])
  }
  collect(definition.model, [], [])
  const composed = composeSchemaGraphs(
    codecs.map((codec) => codec.graph),
    `tool-${definition.name}`,
  )
  const enc = new GraphEncoder(composed.defs)
  const rootByCodec = new Map(codecs.map((codec, index) => [codec, composed.roots[index]]))
  const typeIndices = new Map<CompiledWitCodec<any>, number>()
  const indexFor = (codec: CompiledWitCodec<any>) => {
    let index = typeIndices.get(codec)
    if (index === undefined) {
      index = enc.encodeType(rootByCodec.get(codec)!)
      typeIndices.set(codec, index)
    }
    return index
  }
  const visit = (m: CommandModel, path: string[], inherited: readonly ArgumentSpec[]): number => {
    const index = nodes.length
    nodes.push(undefined as never)
    let cb: CompiledBody | undefined
    const localGlobals = m.body?.args.filter((a) => a.global) ?? []
    if (m.body) {
      const args = ordered([...inherited, ...m.body.args])
      const input = inputCodecs.get(path.join("/"))!
      const output = m.body.output ? compileOnce(m.body.output) : undefined
      const errors = m.body.errors.map((spec) => {
        const codec = compileOnce(spec.schema)
        return { spec, codec }
      })
      cb = { model: m.body, args, input, output, errors }
      bodies.set(path.join("/"), cb)
    }
    const children = Object.values(m.children).map((c) =>
      visit(c, [...path, c.name], [...inherited, ...localGlobals]),
    )
    nodes[index] = {
      name: m.name,
      aliases: [...m.aliases],
      doc: doc(m.doc),
      globals: globalsWire(localGlobals),
      subcommands: children,
      body: cb ? bodyWire(cb) : undefined,
    }
    return index
  }
  const bodyWire = (b: CompiledBody): ToolCommon.CommandBody => {
    const positionals: ToolCommon.Positional[] = []
    const options: ToolCommon.OptionSpec[] = []
    const flags: ToolCommon.FlagSpec[] = []
    let tail: ToolCommon.TailPositional | undefined
    for (const a of b.args.filter((a) => !a.global)) {
      if (a.kind === "flag" && a.flag === "count") {
        const max = (a.options as ArgumentOptions & { readonly max?: number }).max
        if (max !== undefined && (!Number.isInteger(max) || max < 0 || max > 0xffffffff)) {
          throw new Error(`count flag '${a.name}' maximum must be an integer in 0..4294967295`)
        }
      }
      const fieldCodec = compileOnce(a.wireSchema)
      const ti = indexFor(fieldCodec)
      const d = doc(a.options.doc)
      if (a.kind === "positional")
        positionals.push({
          name: a.name,
          doc: d,
          type: ti,
          default_:
            a.options.default === undefined
              ? undefined
              : Effect.runSync(fieldCodec.encode(a.options.default) as Effect.Effect<any, any>),
          required: a.options.required ?? true,
          acceptsStdio: a.options.acceptsStdio ?? false,
          valueName: a.options.valueName,
        })
      else if (a.kind === "tail")
        tail = {
          name: a.name,
          doc: d,
          itemType: ti,
          min: (a.options as any).min ?? 0,
          max: (a.options as any).max,
          separator: (a.options as any).separator,
          verbatim: (a.options as any).verbatim ?? false,
          acceptsStdio: a.options.acceptsStdio ?? false,
          valueName: a.options.valueName,
        }
      else if (a.kind === "flag")
        flags.push({
          long: a.name,
          short: a.options.short,
          aliases: [...(a.options.aliases ?? [])],
          doc: d,
          shape:
            a.flag === "count"
              ? { tag: "count-flag", val: (a.options as any).max }
              : {
                  tag: "bool-flag",
                  val: {
                    default_: (a.options.default as boolean) ?? false,
                    negatable: (a.options as any).negatable ?? false,
                  },
                },
          envVar: a.options.envVar,
        })
      else {
        const isMap = a.repeatable && resolveRoot(fieldCodec).tag === "map"
        options.push({
          long: a.name,
          short: a.options.short,
          aliases: [...(a.options.aliases ?? [])],
          doc: d,
          shape: a.repeatable
            ? isMap
              ? {
                  tag: "repeatable-map",
                  val: {
                    repetition: repetition(a.repeatable, a.options.delim),
                    mapType: ti,
                    duplicateKeyPolicy: a.options.duplicateKeyPolicy ?? "reject",
                  },
                }
              : {
                  tag: "repeatable-list",
                  val: { repetition: repetition(a.repeatable, a.options.delim), itemType: ti },
                }
            : a.options.optionalScalar
              ? { tag: "optional-scalar", val: ti }
              : { tag: "scalar", val: ti },
          default_:
            a.options.default === undefined
              ? undefined
              : Effect.runSync(fieldCodec.encode(a.options.default) as Effect.Effect<any, any>),
          required: a.options.required ?? false,
          envVar: a.options.envVar ?? a.options.env,
          valueName: a.options.valueName,
        })
      }
    }
    return {
      positionals: { fixed: positionals, tail },
      options,
      flags,
      constraints: [...b.model.constraints],
      stdin: b.model.stdin
        ? {
            doc: doc(b.model.stdin.doc),
            mime: [...(b.model.stdin.mime ?? [])],
            required: b.model.stdin.required ?? false,
          }
        : undefined,
      stdout: b.model.stdout
        ? {
            doc: doc(b.model.stdout.doc),
            mime: [...(b.model.stdout.mime ?? [])],
            required: b.model.stdout.required ?? false,
          }
        : undefined,
      result: b.output
        ? {
            type: indexFor(b.output),
            doc: doc(b.model.resultOptions?.doc),
            formatters: (b.model.resultOptions?.formatters ?? []).map((f) =>
              typeof f === "string" ? { name: f, doc: doc() } : f,
            ),
            defaultFormatter: b.model.resultOptions?.defaultFormatter ?? "",
          }
        : undefined,
      errors: b.errors.map(({ spec, codec }) => ({
        name: spec.name,
        doc: doc(spec.options.doc),
        kind: spec.options.kind ?? "runtime-error",
        exitCode: spec.options.exitCode ?? 1,
        payload: indexFor(codec),
      })),
      annotations: b.model.annotations
        ? {
            readOnly: b.model.annotations.readOnly ?? false,
            destructive: b.model.annotations.destructive ?? true,
            idempotent: b.model.annotations.idempotent ?? false,
            openWorld: b.model.annotations.openWorld ?? true,
          }
        : undefined,
    }
  }
  const globalsWire = (args: readonly ArgumentSpec[]): ToolCommon.Globals => {
    const encoded = bodyWire({
      model: { fields: {}, args, errors: [], constraints: [] },
      args: args.map((a) => ({ ...a, global: false })),
      input: undefined as never,
      errors: [],
    })
    return { options: encoded.options, flags: encoded.flags }
  }
  visit(definition.model, [], [])
  return {
    definition,
    bodies,
    wire: { version: "0.1.0", commands: { nodes }, schema: enc.finish() },
  }
}

const repetition = (mode: RepeatableMode, delimiter?: string): ToolCommon.Repetition => {
  if (mode === "repeated") return { tag: "repeated" }
  if (mode === "delimited" || mode === "either") {
    if (delimiter === undefined) throw new Error(`${mode} repeatable options require a delimiter`)
    return { tag: mode, val: delimiter }
  }
  return rep(mode)
}

const resolveRoot = (codec: CompiledWitCodec<any>) => {
  let body = codec.graph.root.body
  const seen = new Set<string>()
  while (body.tag === "ref") {
    if (seen.has(body.id))
      throw new Error(`recursive reference '${body.id}' cannot be an option map`)
    seen.add(body.id)
    const definition = codec.graph.defs.get(body.id)
    if (!definition) throw new Error(`unresolved schema reference '${body.id}'`)
    body = definition.body.body
  }
  return body
}

const identifier = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/
function validateDefinition(root: CommandModel): void {
  const visit = (
    command: CommandModel,
    inheritedNames: Set<string>,
    inheritedShorts: Set<string>,
  ) => {
    if (!identifier.test(command.name)) throw new Error(`invalid command name '${command.name}'`)
    const siblingNames = new Set<string>()
    for (const child of Object.values(command.children)) {
      for (const name of [child.name, ...child.aliases]) {
        if (!identifier.test(name)) throw new Error(`invalid command name or alias '${name}'`)
        if (siblingNames.has(name)) throw new Error(`duplicate subcommand name or alias '${name}'`)
        siblingNames.add(name)
      }
    }
    const names = new Set(inheritedNames)
    const shorts = new Set(inheritedShorts)
    for (const argument of command.body?.args ?? []) {
      for (const name of [argument.name, ...(argument.options.aliases ?? [])]) {
        if (!identifier.test(name)) throw new Error(`invalid argument name or alias '${name}'`)
        if (names.has(name)) throw new Error(`duplicate argument name or alias '${name}'`)
        names.add(name)
      }
      if (argument.options.short) {
        if ([...argument.options.short].length !== 1)
          throw new Error(`short form '${argument.options.short}' must be one character`)
        if (shorts.has(argument.options.short))
          throw new Error(`duplicate argument short form '${argument.options.short}'`)
        shorts.add(argument.options.short)
      }
      if (argument.kind === "tail") {
        const options = argument.options as ArgumentOptions & {
          readonly min?: number
          readonly max?: number
          readonly separator?: string
          readonly verbatim?: boolean
        }
        if (options.verbatim && options.separator === undefined)
          throw new Error(`verbatim tail '${argument.name}' requires a separator`)
        if (
          (options.min ?? 0) < 0 ||
          (options.max !== undefined && options.max < (options.min ?? 0))
        )
          throw new Error(`invalid bounds for tail '${argument.name}'`)
      }
    }
    const resolvable = names
    for (const constraint of command.body?.constraints ?? []) {
      const refs =
        constraint.tag === "mutex-groups"
          ? constraint.val.flatMap((group) => group.refs)
          : constraint.tag === "implies" || constraint.tag === "forbids"
            ? [...constraint.val.lhs, ...constraint.val.rhs]
            : constraint.val
      for (const ref of refs)
        if (!resolvable.has(ref.val instanceof Object ? ref.val.name : ref.val))
          throw new Error(
            `constraint references unknown argument '${ref.tag === "present" ? ref.val : ref.val.name}'`,
          )
    }
    const formatters =
      command.body?.resultOptions?.formatters?.map((f) => (typeof f === "string" ? f : f.name)) ??
      []
    const defaultFormatter = command.body?.resultOptions?.defaultFormatter
    if (defaultFormatter !== undefined && !formatters.includes(defaultFormatter))
      throw new Error(`default formatter '${defaultFormatter}' is not declared`)
    const globalNames = new Set(inheritedNames)
    const globalShorts = new Set(inheritedShorts)
    for (const argument of command.body?.args.filter((a) => a.global) ?? []) {
      globalNames.add(argument.name)
      for (const alias of argument.options.aliases ?? []) globalNames.add(alias)
      if (argument.options.short) globalShorts.add(argument.options.short)
    }
    for (const child of Object.values(command.children)) visit(child, globalNames, globalShorts)
  }
  visit(root, new Set(), new Set())
}

function registerTool<N extends string>(
  definition: ToolDefinition<N>,
  implementation: ToolImplementation,
  layer?: Layer.Layer<any>,
): ImplementedTool<N> {
  if (registry.has(definition.name))
    throw new Error(`Tool '${definition.name}' is already registered`)
  const compiled = compileDefinition(definition)
  for (const path of compiled.bodies.keys()) {
    if (!implementationAt(implementation, definition.name, path ? path.split("/") : []))
      throw new Error(`missing implementation for tool command '${path || definition.name}'`)
  }
  registry.set(definition.name, { ...compiled, implementation, layer })
  return { name: definition.name, definition }
}
export const registeredTools = () => [...registry.values()]
export const resetTools = () => registry.clear()
export const findCommand = (r: Registered, path: readonly string[]) => r.bodies.get(path.join("/"))
/** Canonical host input order: inherited globals, positionals, tail, options, then flags. */
export const canonicalInputFields = (
  definition: ToolDefinition,
  path: readonly string[],
): Fields | undefined => {
  let command = definition.model
  const inherited: ArgumentSpec[] = []
  for (const segment of path) {
    inherited.push(...(command.body?.args.filter((argument) => argument.global) ?? []))
    const next = command.children[segment]
    if (!next) return undefined
    command = next
  }
  if (!command.body) return undefined
  const args = [...inherited, ...command.body.args]
  const ordered = [
    ...args.filter((a) => a.kind === "option" && a.global),
    ...args.filter((a) => a.kind === "flag" && a.global),
    ...args.filter((a) => a.kind === "positional"),
    ...args.filter((a) => a.kind === "tail"),
    ...args.filter((a) => a.kind === "option" && !a.global),
    ...args.filter((a) => a.kind === "flag" && !a.global),
  ]
  return Object.fromEntries(ordered.map((argument) => [argument.name, argument.schema]))
}
export const implementationAt = (
  impl: ToolImplementation,
  root: string,
  path: readonly string[],
): Handler<any, any> | undefined => {
  let value: Handler<any, any> | ToolImplementation | undefined =
    path.length === 0 ? impl[root.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())] : impl
  for (const segment of path)
    value =
      typeof value === "object"
        ? value[segment.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())]
        : undefined
  return typeof value === "function" ? value : undefined
}
export const c = {
  present: (name: string): ToolCommon.Ref => ({ tag: "present", val: name }),
  valueIs: (name: string, value: Core.SchemaValueTree): ToolCommon.Ref => ({
    tag: "value-is",
    val: { name, value },
  }),
  requiresAll: (...refs: ToolCommon.Ref[]): ToolCommon.Constraint => ({
    tag: "requires-all",
    val: refs,
  }),
  allOrNone: (...refs: ToolCommon.Ref[]): ToolCommon.Constraint => ({
    tag: "all-or-none",
    val: refs,
  }),
  requiresAny: (...refs: ToolCommon.Ref[]): ToolCommon.Constraint => ({
    tag: "requires-any",
    val: refs,
  }),
  mutex: (...groups: readonly ToolCommon.Ref[][]): ToolCommon.Constraint => ({
    tag: "mutex-groups",
    val: groups.map((refs) => ({ refs: [...refs] })),
  }),
  implies: (
    lhs: ToolCommon.Ref | readonly ToolCommon.Ref[],
    rhs: ToolCommon.Ref | readonly ToolCommon.Ref[],
    lhsQuant: ToolCommon.Quantifier = "all",
    rhsQuant: ToolCommon.Quantifier = "all",
  ): ToolCommon.Constraint => ({
    tag: "implies",
    val: {
      lhsQuant,
      lhs: Array.isArray(lhs) ? [...lhs] : [lhs as ToolCommon.Ref],
      rhsQuant,
      rhs: Array.isArray(rhs) ? [...rhs] : [rhs as ToolCommon.Ref],
    },
  }),
  forbids: (
    lhs: ToolCommon.Ref | readonly ToolCommon.Ref[],
    rhs: ToolCommon.Ref | readonly ToolCommon.Ref[],
    lhsQuant: ToolCommon.Quantifier = "all",
  ): ToolCommon.Constraint => ({
    tag: "forbids",
    val: {
      lhsQuant,
      lhs: Array.isArray(lhs) ? [...lhs] : [lhs as ToolCommon.Ref],
      rhs: Array.isArray(rhs) ? [...rhs] : [rhs as ToolCommon.Ref],
    },
  }),
}
export const renderHelp = (definition: ToolDefinition, path: readonly string[] = []): string => {
  let command = definition.model
  for (const p of path) {
    const next = Object.values(command.children).find((c) => c.name === p || c.aliases.includes(p))
    if (!next) throw new Error(`Unknown command: ${p}`)
    command = next
  }
  const usage = [
    definition.name,
    ...path,
    command.body ? "[OPTIONS]" : "",
    Object.keys(command.children).length ? "<COMMAND>" : "",
  ]
    .filter(Boolean)
    .join(" ")
  return [
    `Usage: ${usage}`,
    doc(command.doc).summary,
    ...Object.values(command.children).map((x) => `  ${x.name}\t${doc(x.doc).summary}`),
  ]
    .filter(Boolean)
    .join("\n")
}
