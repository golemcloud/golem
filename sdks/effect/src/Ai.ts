/** Optional Effect AI integration for explicitly selected Golem tools. @since 1.6.0 */
import { Cause, Effect, Exit, Schema, SchemaAST, Stream } from "effect"
import { Tool as EffectAiTool, Toolkit as EffectAiToolkit } from "effect/unstable/ai"
import {
  Reflection as GolemReflection,
  Tool as GolemTool,
  WitCodec,
} from "@golemcloud/effect-golem"
import type { JsonValue } from "./SchemaRef.js"
import { type BodyModel, type CommandModel, type ToolDefinition } from "./internal/tool/model.js"

/** Model-provided bytes for a command's stdin stream. @since 1.6.0 @category models */
export interface AiStdin {
  readonly data: string
  readonly encoding: "utf8" | "base64"
}

/** Bounded stdout returned to the model after the stream is fully drained. @since 1.6.0 @category models */
export interface CapturedStdout {
  readonly data: string
  readonly encoding: "utf8" | "base64"
  readonly truncated: boolean
  readonly totalBytes: number
}

/** Model-visible outcome of a Golem tool invocation. @since 1.6.0 @category models */
export type GolemAiToolResult =
  | {
      readonly status: "success"
      readonly result?: JsonValue
      readonly stdout?: CapturedStdout
    }
  | {
      readonly status: "error"
      readonly error: {
        readonly name: string
        readonly value?: JsonValue
      }
      readonly stdout?: CapturedStdout
    }

/** Approval policy forwarded to Effect AI tool definitions. @since 1.6.0 @category models */
export type NeedsApproval =
  | boolean
  | ((
      parameters: Readonly<Record<string, unknown>>,
      context: EffectAiTool.NeedsApprovalContext,
    ) => boolean | Effect.Effect<boolean>)

/** Adapter-owned options for one explicitly selected command. @since 1.6.0 @category models */
export interface CommandOptions {
  readonly name?: string
  readonly maxStdoutBytes?: number
  readonly needsApproval?: NeedsApproval
}

/** An explicitly selected root or nested Golem command. @since 1.6.0 @category models */
export interface CommandSelection {
  readonly path: ReadonlyArray<string>
  readonly options: Readonly<CommandOptions>
}

/** A selection with its collision-checked model-facing name. @since 1.6.0 @category models */
export interface ResolvedCommandSelection extends CommandSelection {
  readonly name: string
}

/** An explicitly selected runtime-reflected command. @since 1.6.0 @category models */
export interface ReflectedCommandSelection {
  readonly command: GolemReflection.ToolCommand
  readonly options: Readonly<CommandOptions>
}

/** A command or stream documentation fragment used in model-facing descriptions. @since 1.6.0 @category models */
export interface Documentation {
  readonly summary?: string
  readonly description?: string
}

/** Stream metadata used to document synthetic stdin and captured stdout. @since 1.6.0 @category models */
export interface StreamDocumentation {
  readonly doc?: Documentation
  readonly mime?: ReadonlyArray<string>
  readonly required?: boolean
}

/** Client binding used by the typed adapter, including explicit registration lookup names. @since 1.6.0 @category models */
export type TypedToolkitOptions = GolemTool.ClientOptions

type BuiltAiTool = EffectAiTool.Tool<
  string,
  {
    readonly parameters: typeof Schema.Unknown
    readonly success: typeof Schema.Unknown
    readonly failure: typeof Schema.Never
    readonly failureMode: "error"
  }
>

/** Select one command path for model exposure. No ambient tools are selected implicitly. @since 1.6.0 @category constructors */
export const command = (
  path: ReadonlyArray<string> = [],
  options: CommandOptions = {},
): CommandSelection => {
  const normalizedPath = path.map((segment) => {
    if (segment.length === 0) throw new TypeError("AI command path segments cannot be empty")
    return segment
  })
  validateCommandOptions(options)
  return Object.freeze({
    path: Object.freeze(normalizedPath),
    options: Object.freeze({ ...options }),
  })
}

/** Select an already reflected command for model exposure. @since 1.6.0 @category constructors */
export const reflectedCommand = (
  reflected: GolemReflection.ToolCommand,
  options: CommandOptions = {},
): ReflectedCommandSelection => {
  validateCommandOptions(options)
  return Object.freeze({ command: reflected, options: Object.freeze({ ...options }) })
}

const validateCommandOptions = (options: CommandOptions): void => {
  if (options.name !== undefined) validateAiName(options.name)
  if (
    options.maxStdoutBytes !== undefined &&
    (!Number.isSafeInteger(options.maxStdoutBytes) || options.maxStdoutBytes < 0)
  )
    throw new TypeError("maxStdoutBytes must be a non-negative safe integer")
}

/** Resolve deterministic AI names and reject collisions before model invocation. @since 1.6.0 @category constructors */
export const resolveCommandSelections = (
  toolName: string,
  selections: ReadonlyArray<CommandSelection>,
): ReadonlyArray<ResolvedCommandSelection> => {
  if (toolName.length === 0) throw new TypeError("Golem tool names cannot be empty")
  const names = new Set<string>()
  return Object.freeze(
    selections.map((selection) => {
      validateCommandOptions(selection.options)
      const options = Object.freeze({ ...selection.options })
      const name = options.name ?? [toolName, ...selection.path].join("__")
      validateAiName(name)
      if (names.has(name)) throw new TypeError(`duplicate AI tool name '${name}'`)
      names.add(name)
      return Object.freeze({ ...selection, options, name })
    }),
  )
}

/** Build command and stream documentation for the model-facing tool description. @since 1.6.0 @category conversions */
export const commandDescription = (
  doc: Documentation | undefined,
  stdin?: StreamDocumentation,
  stdout?: StreamDocumentation,
): string | undefined => {
  const sections = documentationLines(doc)
  if (stdin) {
    sections.push(
      `Stdin is ${stdin.required ? "required" : "optional"}; pass _stdin as { data, encoding: "utf8" | "base64" }.${mimeHint(stdin)}`,
      ...documentationLines(stdin.doc),
    )
  }
  if (stdout) {
    sections.push(
      `Stdout is returned as bounded captured bytes after the stream is fully drained.${mimeHint(stdout)}`,
      ...documentationLines(stdout.doc),
    )
  }
  return sections.length === 0 ? undefined : sections.join("\n\n")
}

/** Build an Effect AI toolkit for explicitly selected commands in a typed Golem definition. @since 1.6.0 @category constructors */
export function typedToolkit<D extends ToolDefinition>(
  definition: D,
  selections: ReadonlyArray<CommandSelection>,
  options: TypedToolkitOptions & { readonly transport: GolemTool.ToolTransport },
): Effect.Effect<EffectAiToolkit.WithHandler<Record<string, BuiltAiTool>>>
export function typedToolkit<D extends ToolDefinition>(
  definition: D,
  selections: ReadonlyArray<CommandSelection>,
  options?: TypedToolkitOptions,
): Effect.Effect<
  EffectAiToolkit.WithHandler<Record<string, BuiltAiTool>>,
  never,
  GolemTool.ToolClient
>
export function typedToolkit<D extends ToolDefinition>(
  definition: D,
  selections: ReadonlyArray<CommandSelection>,
  options: TypedToolkitOptions = {},
) {
  const resolved = resolveCommandSelections(definition.name, selections)
  const projected = GolemTool.client(definition, options)
  const tools: Array<BuiltAiTool> = []
  const handlers: Record<
    string,
    (parameters: Record<string, unknown>) => Effect.Effect<unknown, unknown, unknown>
  > = {}

  for (const selection of resolved) {
    const model = commandAt(definition.model, selection.path)
    if (!model?.body)
      throw new TypeError(
        `unknown or non-callable command '${[definition.name, ...selection.path].join(" ")}'`,
      )
    const body = model.body
    const fields = GolemTool.canonicalInputFields(definition, selection.path)!
    assertUniqueProjectedFieldNames(fields, selection.name)
    if (body.stdin && Object.hasOwn(fields, "_stdin"))
      throw new TypeError(`AI command '${selection.name}' parameter '_stdin' conflicts with stdin`)
    const defaults = GolemTool.canonicalInputDefaults(definition, selection.path)!
    const parameterFields: Record<string, Schema.Top> = { ...fields }
    if (body.stdin) {
      parameterFields._stdin = body.stdin.required
        ? AiStdinSchema
        : Schema.optionalKey(AiStdinSchema)
    }
    const parameters = Schema.Struct(parameterFields)
    const parameterCodec = compileJson(parameters, `${selection.name} parameters`)
    const parameterDefaults = Effect.forEach(Object.entries(defaults), ([name, value]) =>
      compileJson(fields[name], `${selection.name} default '${name}'`)
        .encode(value)
        .pipe(Effect.map((encoded) => [name, encoded] as const)),
    ).pipe(Effect.map(Object.fromEntries))
    const optionalParameters = [
      ...Object.keys(defaults),
      ...Object.entries(parameterFields)
        .filter(([, schema]) => SchemaAST.isOptional(schema.ast))
        .map(([name]) => name),
    ]
    const decodeParameters = (value: Readonly<Record<string, unknown>>) =>
      parameterDefaults.pipe(
        Effect.flatMap((defaults) =>
          parameterCodec.decode(normalizeOptionalParameters(value, defaults, optionalParameters)),
        ),
        Effect.map((decoded) => decoded as Readonly<Record<string, unknown>>),
      )
    if (body.stdout && selection.options.maxStdoutBytes === undefined)
      throw new TypeError(`AI command '${selection.name}' requires maxStdoutBytes`)

    const encodeResult = body.output
      ? compileJson(body.output, `${selection.name} result`).encode
      : undefined
    const encodeErrors = new Map(
      body.errors.map((error) => [
        error.name,
        compileJson(error.schema, `${selection.name} error '${error.name}'`).encode,
      ]),
    )
    const tool = EffectAiTool.dynamic(selection.name, {
      description: commandDescription(
        normalizeDoc(model.doc),
        body.stdin ? normalizeStream(body.stdin) : undefined,
        body.stdout ? normalizeStream(body.stdout) : undefined,
      ),
      parameters: omitOptionalRequirements(parameterCodec.jsonSchema, optionalParameters) as never,
      success: Schema.Unknown,
      failure: Schema.Never,
      failureMode: "error",
      needsApproval: normalizeApproval(selection.options.needsApproval, decodeParameters) as never,
    })
    tools.push(tool as unknown as BuiltAiTool)
    handlers[selection.name] = (parameters) =>
      decodeParameters(parameters).pipe(
        Effect.flatMap((decoded) =>
          invokeTyped(
            projected,
            selection.path,
            fields,
            body,
            decoded as Record<string, unknown>,
            selection.options.maxStdoutBytes,
            encodeResult,
            encodeErrors,
          ),
        ),
      )
  }

  const toolkit = EffectAiToolkit.make(...tools)
  const assembled = toolkit
    .toHandlers(handlers as never)
    .pipe(Effect.flatMap((context) => toolkit.pipe(Effect.provide(context))))
  return options.transport
    ? assembled
    : Effect.context<GolemTool.ToolClient>().pipe(Effect.andThen(assembled))
}

/** Build an Effect AI toolkit from selected commands on one reflected tool. @since 1.6.0 @category constructors */
export function reflectedToolkit(
  tool: GolemReflection.ToolType,
  selections: ReadonlyArray<CommandSelection>,
): Effect.Effect<
  EffectAiToolkit.WithHandler<Record<string, BuiltAiTool>>,
  never,
  GolemTool.ToolClient
>
/** Build an Effect AI toolkit from already selected reflected commands. @since 1.6.0 @category constructors */
export function reflectedToolkit(
  selections: ReadonlyArray<ReflectedCommandSelection>,
): Effect.Effect<
  EffectAiToolkit.WithHandler<Record<string, BuiltAiTool>>,
  never,
  GolemTool.ToolClient
>
export function reflectedToolkit(
  toolOrSelections: GolemReflection.ToolType | ReadonlyArray<ReflectedCommandSelection>,
  selections?: ReadonlyArray<CommandSelection>,
) {
  const selected = Array.isArray(toolOrSelections)
    ? (toolOrSelections as ReadonlyArray<ReflectedCommandSelection>).map((selection) => {
        validateCommandOptions(selection.options)
        const options = Object.freeze({ ...selection.options })
        return {
          ...selection,
          options,
          modelName:
            options.name ?? [selection.command.toolName, ...selection.command.path].join("__"),
        }
      })
    : (() => {
        const tool = toolOrSelections as GolemReflection.ToolType
        return resolveCommandSelections(tool.name, selections ?? []).map((selection) => ({
          command: tool.client.command(selection.path),
          options: selection.options,
          modelName: selection.name,
        }))
      })()
  rejectDuplicateNames(selected.map((selection) => selection.modelName))

  const tools: Array<BuiltAiTool> = []
  const handlers: Record<
    string,
    (parameters: Record<string, unknown>) => Effect.Effect<unknown, unknown, unknown>
  > = {}
  for (const selection of selected) {
    const reflected = selection.command
    const inputSchema = reflected.inputSchema
    if (!inputSchema)
      throw new TypeError(
        `unknown or non-callable command '${[reflected.toolName, ...reflected.path].join(" ")}'`,
      )
    assertReflectedJson(inputSchema, `${selection.modelName} parameters`)
    if (reflected.result) assertReflectedJson(reflected.result, `${selection.modelName} result`)
    for (const error of reflected.errors)
      if (error.payload)
        assertReflectedJson(error.payload, `${selection.modelName} error '${error.name}'`)
    if (reflected.stdout && selection.options.maxStdoutBytes === undefined)
      throw new TypeError(`AI command '${selection.modelName}' requires maxStdoutBytes`)

    const optional = reflected.arguments.filter((argument) => !argument.required)
    const optionalNames = optional.map((argument) => argument.name)
    const defaults = new Map(
      optional.map((argument) => {
        if (argument.defaultJson !== undefined)
          return [argument.name, argument.defaultJson] as const
        if (argument.optionalCarrier) return [argument.name, null] as const
        throw new TypeError(`optional reflected argument '${argument.name}' has no default`)
      }),
    )
    const parameterSchema = reflectedParameterSchema(
      inputSchema.toJsonSchema({ includeDraftMarker: false }),
      optionalNames,
      reflected.stdin?.required,
    )
    const decodeParameters = (value: Readonly<Record<string, unknown>>) =>
      validateReflectedParameters(reflected, normalizeReflectedParameters(value, defaults))
    const tool = EffectAiTool.dynamic(selection.modelName, {
      description: commandDescription(
        normalizeReflectedDoc(reflected.doc),
        reflected.stdin ? normalizeReflectedStream(reflected.stdin) : undefined,
        reflected.stdout ? normalizeReflectedStream(reflected.stdout) : undefined,
      ),
      parameters: parameterSchema as never,
      success: Schema.Unknown,
      failure: Schema.Never,
      failureMode: "error",
      needsApproval: normalizeApproval(selection.options.needsApproval, decodeParameters) as never,
    })
    tools.push(tool as unknown as BuiltAiTool)
    handlers[selection.modelName] = (parameters) =>
      invokeReflected(
        reflected,
        normalizeReflectedParameters(parameters, defaults),
        selection.options.maxStdoutBytes,
      )
  }

  const toolkit = EffectAiToolkit.make(...tools)
  const assembled = toolkit
    .toHandlers(handlers as never)
    .pipe(Effect.flatMap((context) => toolkit.pipe(Effect.provide(context))))
  return Effect.context<GolemTool.ToolClient>().pipe(Effect.andThen(assembled))
}

const documentationLines = (doc: Documentation | undefined): string[] =>
  [doc?.summary, doc?.description].filter((value): value is string => !!value)

const mimeHint = (stream: StreamDocumentation): string =>
  stream.mime && stream.mime.length > 0 ? ` Declared MIME hints: ${stream.mime.join(", ")}.` : ""

const AiStdinSchema = Schema.Struct({
  data: Schema.String,
  encoding: Schema.Literals(["utf8", "base64"]),
})

const commandAt = (root: CommandModel, path: ReadonlyArray<string>): CommandModel | undefined => {
  let command: CommandModel | undefined = root
  for (const segment of path) {
    if (!command) return undefined
    const projectedName = camelCase(segment)
    const aliases = Object.keys(command.children).filter(
      (name) => camelCase(name) === projectedName,
    )
    if (aliases.length > 1)
      throw new TypeError(
        `AI command path segment '${segment}' is ambiguous after TypeScript projection: ${aliases.join(", ")}`,
      )
    command = command.children[segment]
  }
  return command
}

const assertUniqueProjectedFieldNames = (
  fields: Readonly<Record<string, Schema.Top>>,
  commandName: string,
): void => {
  const projected = new Map<string, string>()
  for (const name of Object.keys(fields)) {
    const property = camelCase(name)
    const previous = projected.get(property)
    if (previous !== undefined)
      throw new TypeError(
        `AI command '${commandName}' parameters '${previous}' and '${name}' both project to '${property}'`,
      )
    projected.set(property, name)
  }
}

const normalizeDoc = (doc: CommandModel["doc"]): Documentation | undefined =>
  typeof doc === "string" ? { summary: doc } : doc

const normalizeStream = (stream: NonNullable<BodyModel["stdin"]>): StreamDocumentation => ({
  doc: normalizeDoc(stream.doc),
  mime: stream.mime,
  required: stream.required,
})

const normalizeReflectedDoc = (doc: unknown): Documentation | undefined => {
  if (typeof doc === "string") return { summary: doc }
  if (typeof doc !== "object" || doc === null) return undefined
  const value = doc as { readonly summary?: unknown; readonly description?: unknown }
  return {
    ...(typeof value.summary === "string" ? { summary: value.summary } : {}),
    ...(typeof value.description === "string" ? { description: value.description } : {}),
  }
}

const normalizeReflectedStream = (stream: {
  readonly doc: unknown
  readonly mime: ReadonlyArray<string>
  readonly required: boolean
}): StreamDocumentation => ({
  doc: normalizeReflectedDoc(stream.doc),
  mime: stream.mime,
  required: stream.required,
})

const compileJson = (schema: Schema.Top, label: string) => {
  try {
    return Effect.runSync(WitCodec.compileJson(schema))
  } catch (error) {
    throw new TypeError(
      `${label} is not canonical JSON: ${error instanceof Error ? error.message : String(error)}`,
    )
  }
}

const assertReflectedJson = (
  schema: {
    readonly jsonEligibility: () =>
      | { readonly success: true }
      | {
          readonly success: false
          readonly issues: ReadonlyArray<{ readonly message: string }>
        }
  },
  label: string,
): void => {
  const eligibility = schema.jsonEligibility()
  if (!eligibility.success)
    throw new TypeError(`${label} is not canonical JSON: ${eligibility.issues[0]?.message}`)
}

const rejectDuplicateNames = (names: ReadonlyArray<string>): void => {
  const seen = new Set<string>()
  for (const name of names) {
    validateAiName(name)
    if (seen.has(name)) throw new TypeError(`duplicate AI tool name '${name}'`)
    seen.add(name)
  }
}

const validateAiName = (name: string): void => {
  if (name.length === 0) throw new TypeError("AI tool names cannot be empty")
  if (name === "__proto__") throw new TypeError("AI tool name '__proto__' is not supported")
}

const reflectedParameterSchema = (
  schema: JsonValue,
  optional: ReadonlyArray<string>,
  stdinRequired: boolean | undefined,
): JsonValue => {
  if (typeof schema !== "object" || schema === null || Array.isArray(schema))
    throw new TypeError("reflected command input schema must be an object")
  const object = schema as Record<string, JsonValue>
  const properties =
    typeof object.properties === "object" &&
    object.properties !== null &&
    !Array.isArray(object.properties)
      ? (object.properties as Record<string, JsonValue>)
      : {}
  const omitted = new Set(optional)
  const required = Array.isArray(object.required)
    ? object.required.filter((name) => !omitted.has(String(name)))
    : []
  if (stdinRequired) required.push("_stdin")
  return {
    ...object,
    properties: {
      ...properties,
      ...(stdinRequired === undefined
        ? {}
        : {
            _stdin: {
              type: "object",
              properties: {
                data: { type: "string" },
                encoding: { enum: ["utf8", "base64"] },
              },
              required: ["data", "encoding"],
              additionalProperties: false,
            },
          }),
    },
    required,
  }
}

const normalizeReflectedParameters = (
  value: unknown,
  defaults: ReadonlyMap<string, JsonValue>,
): JsonValue => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return value as JsonValue
  const normalized = { ...(value as Record<string, JsonValue>) }
  for (const [name, defaultValue] of defaults)
    if (!Object.hasOwn(normalized, name)) normalized[name] = defaultValue
  return normalized
}

const validateReflectedParameters = (
  command: GolemReflection.ToolCommand,
  value: JsonValue,
): Effect.Effect<Readonly<Record<string, unknown>>, Error> =>
  Effect.try({
    try: () => {
      if (typeof value !== "object" || value === null || Array.isArray(value))
        throw new TypeError("tool parameters must be an object")
      const parameters = value as Record<string, JsonValue>
      if (command.stdin) decodeStdin(parameters._stdin)
      const input = { ...parameters }
      delete input._stdin
      command.packJson(input)
      return parameters
    },
    catch: (cause) =>
      cause instanceof Error ? cause : new TypeError(`invalid tool parameters: ${String(cause)}`),
  })

const normalizeApproval = (
  policy: NeedsApproval | undefined,
  decode: (
    parameters: Readonly<Record<string, unknown>>,
  ) => Effect.Effect<Readonly<Record<string, unknown>>, unknown, unknown>,
): NeedsApproval | undefined => {
  if (typeof policy !== "function") return policy
  return ((parameters, context) =>
    decode(parameters).pipe(
      Effect.flatMap((decoded) => {
        const required = policy(decoded, context)
        return Effect.isEffect(required) ? required : Effect.succeed(required)
      }),
    )) as NeedsApproval
}

const omitOptionalRequirements = (
  schema: JsonValue,
  optional: ReadonlyArray<string>,
): JsonValue => {
  if (
    optional.length === 0 ||
    typeof schema !== "object" ||
    schema === null ||
    Array.isArray(schema)
  )
    return schema
  const object = schema as Record<string, JsonValue>
  const required = object.required
  if (!Array.isArray(required)) return schema
  const omitted = new Set(optional)
  return { ...object, required: required.filter((name) => !omitted.has(String(name))) }
}

const normalizeOptionalParameters = (
  value: unknown,
  defaults: Readonly<Record<string, unknown>>,
  optional: ReadonlyArray<string>,
): JsonValue => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return value as JsonValue
  const normalized = { ...(value as Record<string, JsonValue>) }
  for (const name of optional)
    if (!Object.hasOwn(normalized, name))
      normalized[name] = (Object.hasOwn(defaults, name) ? defaults[name] : null) as JsonValue
  return normalized
}

const invokeReflected = (
  command: GolemReflection.ToolCommand,
  parameters: JsonValue,
  maxStdoutBytes: number | undefined,
): Effect.Effect<unknown, unknown, GolemTool.ToolClient> =>
  Effect.scoped(
    Effect.gen(function* () {
      const object =
        typeof parameters === "object" && parameters !== null && !Array.isArray(parameters)
          ? (parameters as Record<string, JsonValue>)
          : undefined
      const stdin = command.stdin
        ? yield* Effect.try({
            try: () => decodeStdin(object?._stdin),
            catch: (cause) =>
              cause instanceof Error ? cause : new TypeError(`invalid stdin: ${String(cause)}`),
          })
        : undefined
      const input: JsonValue = object ? { ...object } : parameters
      if (command.stdin && typeof input === "object" && input !== null && !Array.isArray(input))
        delete (input as Record<string, JsonValue>)._stdin
      const started = yield* command.startJson(input, stdin)
      let captured: CapturedStdout | undefined = command.stdout
        ? {
            data: "",
            encoding: textualMime(command.stdout.mime) ? "utf8" : "base64",
            truncated: false,
            totalBytes: 0,
          }
        : undefined
      const stdout = command.stdout
        ? captureStdout(started.stdout, maxStdoutBytes!, command.stdout.mime).pipe(
            Effect.tap((value) => Effect.sync(() => (captured = value))),
            Effect.tapError(() => Effect.sync(() => (captured = undefined))),
            Effect.asVoid,
          )
        : Effect.void
      const [resultExit, stdoutExit] = yield* Effect.all(
        [Effect.exit(started.result), Effect.exit(stdout)] as const,
        { concurrency: "unbounded" },
      )
      if (Exit.isFailure(resultExit)) {
        const failure = reflectedFailure(resultExit.cause)
        if (failure)
          return {
            status: "error" as const,
            error: {
              name: failure.name,
              ...(Object.hasOwn(failure, "value") ? { value: failure.value } : {}),
            },
            ...(captured ? { stdout: captured } : {}),
          }
        return yield* Effect.failCause(resultExit.cause)
      }
      if (Exit.isFailure(stdoutExit)) return yield* Effect.failCause(stdoutExit.cause)
      return {
        status: "success" as const,
        ...(command.result ? { result: resultExit.value } : {}),
        ...(command.stdout ? { stdout: captured! } : {}),
      }
    }),
  )

const reflectedFailure = (
  cause: Cause.Cause<unknown>,
): GolemReflection.ReflectedToolFailure | undefined => {
  const reason = cause.reasons.find(Cause.isFailReason)
  if (!reason || typeof reason.error !== "object" || reason.error === null) return undefined
  const failure = reason.error as {
    readonly tag?: unknown
    readonly error?: GolemReflection.ReflectedToolFailure
  }
  return failure.tag === "tool" ? failure.error : undefined
}

const invokeTyped = (
  projected: unknown,
  path: ReadonlyArray<string>,
  fields: Readonly<Record<string, Schema.Top>>,
  body: BodyModel,
  parameters: Record<string, unknown>,
  maxStdoutBytes: number | undefined,
  encodeResult: ((value: unknown) => Effect.Effect<JsonValue, unknown, unknown>) | undefined,
  encodeErrors: ReadonlyMap<string, (value: unknown) => Effect.Effect<JsonValue, unknown, unknown>>,
): Effect.Effect<unknown, unknown, unknown> => {
  const input = Object.fromEntries(
    Object.keys(fields).flatMap((name) =>
      Object.hasOwn(parameters, name) ? [[camelCase(name), parameters[name]]] : [],
    ),
  )
  const invoke = projectedAt(projected, path)
  return Effect.try({
    try: () => (body.stdin ? decodeStdin(parameters._stdin) : undefined),
    catch: (cause) =>
      cause instanceof Error ? cause : new TypeError(`invalid stdin: ${String(cause)}`),
  }).pipe(
    Effect.flatMap((stdin) => {
      let captured: CapturedStdout | undefined = body.stdout
        ? {
            data: "",
            encoding: textualMime(body.stdout.mime) ? "utf8" : "base64",
            truncated: false,
            totalBytes: 0,
          }
        : undefined
      const streams: GolemTool.Streams = {
        ...(stdin ? { stdin } : {}),
        ...(body.stdout
          ? {
              stdout: (source: Stream.Stream<Uint8Array, GolemTool.ToolClientError>) =>
                captureStdout(source, maxStdoutBytes!, body.stdout!.mime).pipe(
                  Effect.tap((value) => Effect.sync(() => (captured = value))),
                  Effect.tapError(() => Effect.sync(() => (captured = undefined))),
                  Effect.asVoid,
                ),
            }
          : {}),
      }
      return invoke(input, streams).pipe(
        Effect.flatMap((result: unknown) =>
          encodeResult ? encodeResult(result) : Effect.succeed(undefined),
        ),
        Effect.map((result) => ({
          status: "success" as const,
          ...(encodeResult ? { result } : {}),
          ...(body.stdout ? { stdout: captured! } : {}),
        })),
        Effect.catchIf(isDeclaredFailure, (failure) => {
          const encode = encodeErrors.get(failure.name)!
          return encode(failure.value).pipe(
            Effect.map((value) => ({
              status: "error" as const,
              error: { name: failure.name, value },
              ...(captured ? { stdout: captured } : {}),
            })),
          )
        }),
      )
    }),
  )
}

const projectedAt = (
  root: unknown,
  path: ReadonlyArray<string>,
): ((input: unknown, streams: GolemTool.Streams) => Effect.Effect<unknown, unknown, unknown>) => {
  let node = root as unknown
  for (const segment of path) node = (node as Readonly<Record<string, unknown>>)[camelCase(segment)]
  return node as (
    input: unknown,
    streams: GolemTool.Streams,
  ) => Effect.Effect<unknown, unknown, unknown>
}

const camelCase = (name: string): string =>
  name.replace(/-([a-z0-9])/g, (_, character: string) => character.toUpperCase())

const isDeclaredFailure = (
  value: unknown,
): value is { readonly _tag: "ToolFailure"; readonly name: string; readonly value: unknown } =>
  typeof value === "object" &&
  value !== null &&
  (value as { readonly _tag?: unknown })._tag === "ToolFailure"

const decodeStdin = (value: unknown): Stream.Stream<Uint8Array> | undefined => {
  if (value === undefined) return undefined
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("stdin must be an object")
  const stdin = value as Partial<AiStdin>
  if (typeof stdin.data !== "string") throw new TypeError("stdin data must be a string")
  if (stdin.encoding !== "utf8" && stdin.encoding !== "base64")
    throw new TypeError("stdin encoding must be 'utf8' or 'base64'")
  const bytes =
    stdin.encoding === "utf8" ? new TextEncoder().encode(stdin.data) : decodeBase64(stdin.data)
  return Stream.make(bytes)
}

const decodeBase64 = (value: string): Uint8Array => {
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value))
    throw new TypeError("invalid base64 stdin")
  const decoded = atob(value)
  if (btoa(decoded) !== value) throw new TypeError("non-canonical base64 stdin")
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0))
}

const captureStdout = <E>(
  source: Stream.Stream<Uint8Array, E>,
  limit: number,
  mime: ReadonlyArray<string> | undefined,
): Effect.Effect<CapturedStdout, E> =>
  Effect.gen(function* () {
    const retained: Uint8Array[] = []
    let retainedBytes = 0
    let totalBytes = 0
    yield* Stream.runForEach(source, (chunk) =>
      Effect.sync(() => {
        totalBytes += chunk.length
        const take = Math.min(chunk.length, limit - retainedBytes)
        if (take > 0) {
          retained.push(chunk.slice(0, take))
          retainedBytes += take
        }
      }),
    )
    const bytes = concatBytes(retained, retainedBytes)
    const text = textualMime(mime) ? strictUtf8(bytes) : undefined
    return {
      data: text ?? encodeBase64(bytes),
      encoding: text === undefined ? "base64" : "utf8",
      truncated: totalBytes > retainedBytes,
      totalBytes,
    }
  })

const concatBytes = (chunks: ReadonlyArray<Uint8Array>, length: number): Uint8Array => {
  const result = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }
  return result
}

const textualMime = (mime: ReadonlyArray<string> | undefined): boolean =>
  mime?.some((declared) => {
    const value = declared.toLowerCase()
    return (
      value.startsWith("text/") ||
      /^(?:application\/)(?:json|.+\+json|xml|.+\+xml|javascript)(?:;|$)/.test(value)
    )
  }) ?? false

const strictUtf8 = (bytes: Uint8Array): string | undefined => {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    return undefined
  }
}

const encodeBase64 = (bytes: Uint8Array): string => {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}
