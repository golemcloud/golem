/** Effect-native runtime agent reflection and schema-checked clients. @since 1.6.0 */
export * from "./ToolReflection.js"
import { Effect, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { AgentId } from "./AgentId.js"
import {
  bindAgentId,
  ClientBindingError,
  connectById,
  connectionAgentId,
  type AgentIdBinding,
  type RemoteCallError,
} from "./Client.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RpcClient, type RpcConnection, type RpcHostError } from "./host/RpcClient.js"
import type { InvocationMetadata } from "./Client.js"
import type { Invocation } from "./InvocationId.js"
import type { InvokeOptions } from "./DynamicClient.js"
import { agentIdToWit } from "./internal/agentId.js"
import { dynamicMethod } from "./internal/dynamicMethod.js"
import { mapInvocation, wrapHostThrow } from "./internal/rpc.js"
import {
  field,
  graphUsesStreams,
  t,
  type SchemaGraph,
  type SchemaType,
} from "./internal/schema-model/model.js"
import { schemaGraphFromWit, schemaGraphToWit } from "./internal/schema-model/wit.js"
import { freezeSchemaGraph, SchemaRef, SchemaRenderError, type JsonValue } from "./SchemaRef.js"

/** A reflected method and its concrete input/output schema roots. @since 1.6.0 @category models */
export interface AgentMethod {
  readonly name: string
  readonly description: string
  readonly promptHint?: string
  readonly input: SchemaRef
  readonly output?: SchemaRef
}

/** Invalid or absent reflected output from a remote agent. @since 1.6.0 @category errors */
export class RemoteOutputError {
  readonly _tag = "RemoteOutputError"
  constructor(readonly reason: string) {}
}

/** Errors exposed by reflected client operations. @since 1.6.0 @category errors */
export type ReflectionError =
  | RemoteCallError
  | SchemaRenderError
  | RemoteOutputError
  | UnknownMethodError
  | ReflectionHostError
  | ReflectionSchemaError

/** A method name absent from the reflected type. @since 1.6.0 @category errors */
export class UnknownMethodError {
  readonly _tag = "UnknownMethodError"
  constructor(
    readonly agentType: string,
    readonly method: string,
  ) {}
}

export class ReflectionHostError {
  readonly _tag = "ReflectionHostError"
  constructor(readonly cause: unknown) {}
}

export class ReflectionSchemaError {
  readonly _tag = "ReflectionSchemaError"
  constructor(readonly cause: unknown) {}
}

/** Result and host invocation metadata returned by reflected calls. @since 1.6.0 @category models */
export interface ReflectedInvocation<T> {
  readonly metadata: InvocationMetadata
  readonly value?: T
}

/** Schema-checked reflected method bound to an agent identity. @since 1.6.0 @category models */
export interface ReflectedMethod {
  readonly definition: AgentMethod
  readonly invoke: (
    input: JsonValue,
    options?: InvokeOptions,
  ) => Effect.Effect<ReflectedInvocation<JsonValue>, ReflectionError>
  readonly invokeValue: (
    input: CoreTypes.SchemaValueTree,
    options?: InvokeOptions,
  ) => Effect.Effect<ReflectedInvocation<CoreTypes.SchemaValueTree>, ReflectionError>
  /**
   * Start the invocation without awaiting its result, and succeed with its handle once the
   * target has durably accepted it. The handle checks the result against the deployed output
   * schema. The current scope owns the handle. Never sends a scope card.
   */
  readonly trigger: (
    input: JsonValue,
  ) => Effect.Effect<ReflectedHandle<JsonValue>, ReflectionError, Scope.Scope>
  readonly triggerValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedHandle<CoreTypes.SchemaValueTree>, ReflectionError, Scope.Scope>
  /**
   * Enqueue the invocation for `at`, and succeed with its handle once the target has durably
   * accepted it. The handle checks the result against the deployed output schema. The current
   * scope owns the handle. Never sends a scope card.
   */
  readonly schedule: (
    at: AgentHost.Datetime,
    input: JsonValue,
  ) => Effect.Effect<ReflectedHandle<JsonValue>, ReflectionError, Scope.Scope>
  readonly scheduleValue: (
    at: AgentHost.Datetime,
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedHandle<CoreTypes.SchemaValueTree>, ReflectionError, Scope.Scope>
}

/**
 * The handle of a reflected invocation. Its result is absent for a unit output.
 *
 * @since 1.6.0
 * @category models
 */
export type ReflectedHandle<T> = Invocation<T | undefined, RemoteOutputError | SchemaRenderError>

/** Reflected client; ephemeral identities arrive in invocation metadata. @since 1.6.0 @category models */
export interface ReflectedAgentClient {
  readonly method: (name: string) => Effect.Effect<ReflectedMethod, ReflectionError>
}

/**
 * Reflected client of one durable agent. `agentId` reads the ID of the agent from the host; on
 * a client of `upsert` it creates the agent when it is missing.
 *
 * @since 1.6.0
 * @category models
 */
export interface ReflectedDurableAgentClient extends ReflectedAgentClient {
  readonly agentId: Effect.Effect<AgentId, ReflectionError>
}

/** Canonical JSON override for a declared local configuration path. @since 1.6.0 @category models */
export interface ReflectedConfigJsonEntry {
  readonly path: ReadonlyArray<string>
  readonly value: JsonValue
}

/** Schema-native override for a declared local configuration path. @since 1.6.0 @category models */
export interface ReflectedConfigValueEntry {
  readonly path: ReadonlyArray<string>
  readonly value: CoreTypes.SchemaValueTree
}

/** A reflected configuration declaration and its selected schema root. @since 1.6.0 @category models */
export interface ReflectedConfigDeclaration {
  readonly path: ReadonlyArray<string>
  readonly source: "local" | "secret"
  readonly schema: SchemaRef
}

/**
 * Options of a reflected `create`. `keyed` creates the keyed agent of the constructor input and
 * fails with a `conflict` cause when it already exists; otherwise the new agent has a new ID.
 * `config` overrides declared local configuration values and `env` adds environment variables.
 *
 * @since 1.6.0
 * @category models
 */
export interface ReflectedCreateOptions<Entry> {
  readonly keyed?: boolean
  readonly config?: ReadonlyArray<Entry>
  readonly env?: Readonly<Record<string, string>>
}

/** Durable reflected client factory. @since 1.6.0 @category models */
export interface DurableClientFactory {
  /** Connect to the keyed agent of the constructor input, creating it when it is missing. */
  readonly upsert: (
    input: JsonValue,
  ) => Effect.Effect<ReflectedDurableAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly upsertValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedDurableAgentClient, ReflectionError, RpcClient | Scope.Scope>
  /** Create an agent and connect to it after its successful initialization. */
  readonly create: (
    input: JsonValue,
    options?: ReflectedCreateOptions<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<ReflectedDurableAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly createValue: (
    input: CoreTypes.SchemaValueTree,
    options?: ReflectedCreateOptions<ReflectedConfigValueEntry>,
  ) => Effect.Effect<ReflectedDurableAgentClient, ReflectionError, RpcClient | Scope.Scope>
  /**
   * Connect to the existing agent with the given ID. An unknown ID or an ID of another agent
   * type fails with a `not-found` cause.
   */
  readonly getById: (
    id: AgentId,
  ) => Effect.Effect<ReflectedDurableAgentClient, ReflectionError, RpcClient | Scope.Scope>
}

/** Ephemeral factory; each invocation runs on a new one-shot instance. @since 1.6.0 @category models */
export interface EphemeralClientFactory {
  readonly upsert: (
    input: JsonValue,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly upsertValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
}

interface AgentTypeMetadata extends AgentIdBinding<
  ReflectedDurableAgentClient,
  ReflectionError | ClientBindingError,
  RpcClient | Scope.Scope
> {
  readonly name: string
  readonly description: string
  readonly sourceLanguage: string
  readonly constructorInput: SchemaRef
  readonly methods: ReadonlyArray<AgentMethod>
  readonly config: ReadonlyArray<ReflectedConfigDeclaration>
  readonly method: (name: string) => AgentMethod | undefined
}

/** Immutable reflected registration, narrowed by lifecycle mode. @since 1.6.0 @category models */
export type AgentType = AgentTypeMetadata &
  (
    | { readonly mode: "durable"; readonly client: DurableClientFactory }
    | { readonly mode: "ephemeral"; readonly client: EphemeralClientFactory }
  )

const schemaError = (error: unknown): SchemaRenderError =>
  error instanceof SchemaRenderError
    ? error
    : new SchemaRenderError([], error instanceof Error ? error.message : String(error))
const pack = (schema: SchemaRef, input: JsonValue) =>
  Effect.suspend(() => {
    const checked = schema.validateJson(input)
    return checked.success
      ? Effect.succeed(checked.value)
      : Effect.fail(
          new SchemaRenderError(
            checked.issues[0]?.path ?? [],
            checked.issues.map((issue) => issue.message).join("; "),
          ),
        )
  })
const wrap = (registration: AgentHost.RegisteredAgentType): AgentType => {
  const raw = structuredClone(registration.agentType)
  const indices = raw.schema.typeNodes.map((_, index) => index)
  const tupleIndex = raw.schema.typeNodes.length
  const decoded = schemaGraphFromWit({
    ...raw.schema,
    typeNodes: [
      ...raw.schema.typeNodes,
      {
        body: { tag: "tuple-type", val: indices },
        metadata: { aliases: [], examples: [] },
      },
    ],
    root: tupleIndex,
  })
  if (decoded.root.body.tag !== "tuple")
    throw new TypeError("reflected schema bundle did not decode to a tuple")
  const roots = decoded.root.body.elements
  const sharedGraph = freezeSchemaGraph({
    defs: decoded.defs,
    root: t.tuple([...roots]),
  } satisfies SchemaGraph)
  const rootAt = (index: number): SchemaType => {
    const root = roots[index]
    if (root === undefined)
      throw new TypeError(`reflected schema type node index out of range: ${index}`)
    return root
  }
  const inputRef = (input: AgentCommon.InputSchema) => {
    const fields = input.val.filter((field) => field.source.tag === "user-supplied")
    return SchemaRef.fromImmutableGraph(
      sharedGraph,
      freezeSchemaGraph(
        t.record(fields.map((item) => field(item.name, rootAt(item.schema), item.metadata))),
      ),
    )
  }
  const methods = Object.freeze(
    raw.methods.map(
      (method): AgentMethod =>
        Object.freeze({
          name: method.name,
          description: method.description,
          ...(method.promptHint === undefined ? {} : { promptHint: method.promptHint }),
          input: inputRef(method.inputSchema),
          ...(method.outputSchema.tag === "unit"
            ? {}
            : {
                output: SchemaRef.fromImmutableGraph(sharedGraph, rootAt(method.outputSchema.val)),
              }),
        }),
    ),
  )
  const constructorInput = inputRef(raw.constructor.inputSchema)
  const config: ReadonlyArray<ReflectedConfigDeclaration> = Object.freeze(
    raw.config.map((declaration) =>
      Object.freeze({
        path: Object.freeze([...declaration.path]),
        source: declaration.source,
        schema: SchemaRef.fromImmutableGraph(sharedGraph, rootAt(declaration.valueType)),
      }),
    ),
  )
  const configDeclaration = (path: ReadonlyArray<string>): ReflectedConfigDeclaration => {
    const found = config.find(
      (declaration) =>
        declaration.path.length === path.length &&
        declaration.path.every((part, index) => part === path[index]),
    )
    if (found === undefined)
      throw new SchemaRenderError(path, `Unknown config path '${path.join(".")}'`)
    if (found.source === "secret")
      throw new SchemaRenderError(
        path,
        `Cannot override secret config field '${path.join(".")}' over RPC`,
      )
    return found
  }
  const packConfigJson = (entries: ReadonlyArray<ReflectedConfigJsonEntry>) =>
    Effect.try({
      try: (): ReadonlyArray<AgentCommon.TypedAgentConfigValue> =>
        entries.map((entry) => {
          const declaration = configDeclaration(entry.path)
          return {
            path: [...entry.path],
            value: {
              graph: schemaGraphToWit(declaration.schema.graph),
              value: declaration.schema.packJson(entry.value),
            },
          }
        }),
      catch: schemaError,
    })
  const validateConfigValues = (entries: ReadonlyArray<ReflectedConfigValueEntry>) =>
    Effect.try({
      try: (): ReadonlyArray<AgentCommon.TypedAgentConfigValue> =>
        entries.map((entry) => {
          const declaration = configDeclaration(entry.path)
          if (!declaration.schema.validateValue(entry.value).success)
            throw new SchemaRenderError(
              entry.path,
              `Invalid config value at '${entry.path.join(".")}'`,
            )
          return {
            path: [...entry.path],
            value: {
              graph: schemaGraphToWit(declaration.schema.graph),
              value: entry.value,
            },
          }
        }),
      catch: schemaError,
    })
  const connected = (acquire: Effect.Effect<RpcConnection, RpcHostError>) =>
    Effect.acquireRelease(
      Effect.mapError(acquire, (error) => wrapHostThrow(error.cause)),
      (connection) => Effect.sync(() => connection.drop()),
    )
  const durableClient = (rpc: RpcConnection): ReflectedDurableAgentClient =>
    Object.freeze({ ...reflectedClient(result, rpc), agentId: connectionAgentId(rpc) })
  const upsertValue = (input: CoreTypes.SchemaValueTree) =>
    Effect.gen(function* () {
      yield* validate(constructorInput, input)
      const host = yield* RpcClient
      return yield* connected(host.upsert(raw.typeName, input))
    })
  const createValue = (
    input: CoreTypes.SchemaValueTree,
    configEntries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
    options: ReflectedCreateOptions<unknown> | undefined,
  ) =>
    Effect.gen(function* () {
      yield* validate(constructorInput, input)
      const host = yield* RpcClient
      const rpc = yield* connected(
        Effect.map(
          host.create(raw.typeName, input, {
            keyed: options?.keyed ?? false,
            agentConfig: [...configEntries],
            env: Object.entries(options?.env ?? {}),
          }),
          ({ connection }) => connection,
        ),
      )
      return durableClient(rpc)
    })
  const getById = (id: AgentId) =>
    Effect.gen(function* () {
      const witId = yield* Effect.try({
        try: () => agentIdToWit(id),
        catch: (cause): RemoteCallError => ({
          _tag: "InvalidUuidError",
          value: String(id),
          reason: String(cause),
        }),
      })
      return durableClient(yield* connectById(raw.typeName, witId))
    })
  const upsertJson = (input: JsonValue) =>
    Effect.flatMap(pack(constructorInput, input), upsertValue)
  const lifecycle =
    raw.mode === "ephemeral"
      ? {
          mode: "ephemeral" as const,
          client: Object.freeze({
            upsert: (input: JsonValue) =>
              Effect.map(upsertJson(input), (rpc) => reflectedClient(result, rpc)),
            upsertValue: (input: CoreTypes.SchemaValueTree) =>
              Effect.map(upsertValue(input), (rpc) => reflectedClient(result, rpc)),
          }) satisfies EphemeralClientFactory,
        }
      : {
          mode: "durable" as const,
          client: Object.freeze({
            upsert: (input) => Effect.map(upsertJson(input), durableClient),
            upsertValue: (input) => Effect.map(upsertValue(input), durableClient),
            create: (input, options) =>
              Effect.flatMap(pack(constructorInput, input), (value) =>
                Effect.flatMap(packConfigJson(options?.config ?? []), (configEntries) =>
                  createValue(value, configEntries, options),
                ),
              ),
            createValue: (input, options) =>
              Effect.flatMap(validateConfigValues(options?.config ?? []), (configEntries) =>
                createValue(input, configEntries, options),
              ),
            getById,
          } satisfies DurableClientFactory),
        }
  const result: AgentType = Object.freeze({
    name: raw.typeName,
    description: raw.description,
    sourceLanguage: raw.sourceLanguage,
    ...lifecycle,
    constructorInput,
    methods,
    config,
    method: (name: string) => methods.find((method) => method.name === name),
    [bindAgentId]: (id: AgentId) =>
      raw.mode === "ephemeral"
        ? Effect.fail(
            new ClientBindingError(
              `Cannot bind agent ID '${id}' to ephemeral agent type '${raw.typeName}'; use upsert(...)`,
            ),
          )
        : getById(id),
  })
  return result
}

const validate = (
  schema: SchemaRef,
  value: CoreTypes.SchemaValueTree,
): Effect.Effect<void, SchemaRenderError> => {
  const validation = schema.validateValue(value)
  return validation.success
    ? Effect.void
    : Effect.fail(
        new SchemaRenderError(
          validation.issues[0]?.path ?? [],
          validation.issues.map((issue) => issue.message).join("; "),
        ),
      )
}

const reflectedClient = (type: AgentType, rpc: RpcConnection): ReflectedAgentClient =>
  Object.freeze({
    method: (name: string) => {
      const definition = type.method(name)
      if (definition === undefined) return Effect.fail(new UnknownMethodError(type.name, name))
      const method = dynamicMethod(rpc, name)
      const streaming =
        graphUsesStreams(definition.input.graph, definition.input.root) ||
        (definition.output !== undefined &&
          graphUsesStreams(definition.output.graph, definition.output.root))
      const notStreaming: Effect.Effect<void, RemoteCallError> = streaming
        ? Effect.fail({
            _tag: "RemoteResponseError",
            reason: `${name}: live streams require invoke-and-await`,
          })
        : Effect.void
      const checkOutput = (
        value: CoreTypes.SchemaValueTree | undefined,
      ): Effect.Effect<CoreTypes.SchemaValueTree | undefined, RemoteOutputError> => {
        if (definition.output === undefined)
          return value === undefined
            ? Effect.succeed(value)
            : Effect.fail(new RemoteOutputError(`${name}: expected unit output`))
        if (value === undefined)
          return Effect.fail(new RemoteOutputError(`${name}: expected an output value`))
        const checked = definition.output.validateValue(value)
        return checked.success
          ? Effect.succeed(value)
          : Effect.fail(
              new RemoteOutputError(
                `${name}: ${checked.issues.map((issue) => issue.message).join("; ")}`,
              ),
            )
      }
      const unpackOutput = (value: CoreTypes.SchemaValueTree | undefined) =>
        value === undefined
          ? Effect.succeed(undefined)
          : Effect.try({ try: () => definition.output!.unpackJson(value), catch: schemaError })
      const checkedHandle = (
        invocation: Invocation<CoreTypes.SchemaValueTree | undefined>,
      ): ReflectedHandle<CoreTypes.SchemaValueTree> => mapInvocation(invocation, checkOutput)
      const jsonHandle = (
        invocation: ReflectedHandle<CoreTypes.SchemaValueTree>,
      ): ReflectedHandle<JsonValue> => mapInvocation(invocation, unpackOutput)
      const invokeValue = (input: CoreTypes.SchemaValueTree, options?: InvokeOptions) =>
        validate(definition.input, input).pipe(
          Effect.flatMap(() => method.invoke(input, options)),
          Effect.tap((invocation) => checkOutput(invocation.value)),
        )
      const reflected: ReflectedMethod = Object.freeze({
        definition,
        invokeValue,
        invoke: (input: JsonValue, options?: InvokeOptions) =>
          pack(definition.input, input).pipe(
            Effect.flatMap((value) => invokeValue(value, options)),
            Effect.flatMap((result) =>
              result.value === undefined
                ? Effect.succeed({ metadata: result.metadata })
                : Effect.try({
                    try: () => ({
                      metadata: result.metadata,
                      value: definition.output!.unpackJson(result.value!),
                    }),
                    catch: schemaError,
                  }),
            ),
          ),
        triggerValue: (input: CoreTypes.SchemaValueTree) =>
          notStreaming.pipe(
            Effect.flatMap(() => validate(definition.input, input)),
            Effect.flatMap(() => method.trigger(input)),
            Effect.map(checkedHandle),
          ),
        trigger: (input: JsonValue) =>
          notStreaming.pipe(
            Effect.flatMap(() => pack(definition.input, input)),
            Effect.flatMap((value) => method.trigger(value)),
            Effect.map((invocation) => jsonHandle(checkedHandle(invocation))),
          ),
        scheduleValue: (at: AgentHost.Datetime, input: CoreTypes.SchemaValueTree) =>
          notStreaming.pipe(
            Effect.flatMap(() => validate(definition.input, input)),
            Effect.flatMap(() => method.schedule(at, input)),
            Effect.map(checkedHandle),
          ),
        schedule: (at: AgentHost.Datetime, input: JsonValue) =>
          notStreaming.pipe(
            Effect.flatMap(() => pack(definition.input, input)),
            Effect.flatMap((value) => method.schedule(at, value)),
            Effect.map((invocation) => jsonHandle(checkedHandle(invocation))),
          ),
      })
      return Effect.succeed(reflected)
    },
  })

/** Discover every visible agent type. @since 1.6.0 @category discovery */
export const getAllAgentTypes: Effect.Effect<
  ReadonlyArray<AgentType>,
  ReflectionHostError | ReflectionSchemaError,
  AgentHostClient
> = Effect.gen(function* () {
  const host = yield* AgentHostClient
  const registrations = yield* Effect.try({
    try: () => host.getAllAgentTypes(),
    catch: (cause) => new ReflectionHostError(cause),
  })
  return yield* Effect.try({
    try: () => Object.freeze(registrations.map(wrap)),
    catch: (cause) => new ReflectionSchemaError(cause),
  })
})

/** Optionally discover a deployed agent type by name. @since 1.6.0 @category discovery */
export const getAgentType = (
  name: string,
): Effect.Effect<
  AgentType | undefined,
  ReflectionHostError | ReflectionSchemaError,
  AgentHostClient
> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    const found = yield* Effect.try({
      try: () => host.getAgentType(name),
      catch: (cause) => new ReflectionHostError(cause),
    })
    if (found === undefined) return undefined
    return yield* Effect.try({
      try: () => wrap(found),
      catch: (cause) => new ReflectionSchemaError(cause),
    })
  })

/**
 * Optionally discover the deployed agent type that the type tag of an agent ID names. It
 * returns `undefined` when no deployed agent type has that tag; it does not check that an agent
 * with the ID exists.
 *
 * @since 1.6.0
 * @category discovery
 */
export const getAgentTypeByAgentId = (
  id: AgentId,
): Effect.Effect<
  AgentType | undefined,
  ReflectionHostError | ReflectionSchemaError,
  AgentHostClient
> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    const found = yield* Effect.try({
      try: () => host.getAgentTypeByAgentId(agentIdToWit(id)),
      catch: (cause) => new ReflectionHostError(cause),
    })
    if (found === undefined) return undefined
    return yield* Effect.try({
      try: () => wrap(found),
      catch: (cause) => new ReflectionSchemaError(cause),
    })
  })
