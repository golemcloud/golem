/** Effect-native runtime agent reflection and schema-checked clients. @since 1.6.0 */
import { Effect, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import * as CoreTypes from "golem:core/types@2.0.0"
import { uuidToString } from "golem:core/types@2.0.0"
import * as AgentIdentity from "./AgentIdentity.js"
import {
  bindIdentity,
  ClientBindingError,
  type IdentityBinding,
  type RemoteCallError,
} from "./Client.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RpcClient, type RpcConnection } from "./host/RpcClient.js"
import { dynamicMethod } from "./internal/dynamicMethod.js"
import { wrapHostThrow } from "./internal/rpc.js"
import { field, t, type SchemaGraph, type SchemaType } from "./internal/schema-model/model.js"
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
  | AgentIdentity.AgentIdentityError
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

export { AgentIdentityError } from "./AgentIdentity.js"

/** Agent-type discovery failed at the host boundary. @since 1.6.0 @category errors */
export class ReflectionHostError {
  readonly _tag = "ReflectionHostError"
  constructor(readonly cause: unknown) {}
}

/** A discovered registration contains a malformed schema graph. @since 1.6.0 @category errors */
export class ReflectionSchemaError {
  readonly _tag = "ReflectionSchemaError"
  constructor(readonly cause: unknown) {}
}

/** Result and host invocation metadata returned by reflected calls. @since 1.6.0 @category models */
export interface ReflectedInvocation<T> {
  readonly metadata: AgentHost.InvocationMetadata
  readonly value?: T
}

/** Schema-checked reflected method bound to an agent identity. @since 1.6.0 @category models */
export interface ReflectedMethod {
  readonly definition: AgentMethod
  readonly invoke: (
    input: JsonValue,
  ) => Effect.Effect<ReflectedInvocation<JsonValue>, ReflectionError>
  readonly invokeValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedInvocation<CoreTypes.SchemaValueTree>, ReflectionError>
  readonly trigger: (
    input: JsonValue,
  ) => Effect.Effect<AgentHost.InvocationMetadata, ReflectionError>
  readonly triggerValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<AgentHost.InvocationMetadata, ReflectionError>
  readonly schedule: (
    at: AgentHost.Datetime,
    input: JsonValue,
  ) => Effect.Effect<ReflectedScheduledInvocation, ReflectionError, Scope.Scope>
  readonly scheduleValue: (
    at: AgentHost.Datetime,
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedScheduledInvocation, ReflectionError, Scope.Scope>
}

/** A cancelable reflected scheduled invocation. @since 1.6.0 @category models */
export interface ReflectedScheduledInvocation {
  readonly metadata: AgentHost.InvocationMetadata
  readonly cancel: Effect.Effect<void>
}

/** Reflected client; ephemeral identities arrive in invocation metadata. @since 1.6.0 @category models */
export interface ReflectedAgentClient {
  readonly method: (name: string) => Effect.Effect<ReflectedMethod, ReflectionError>
}

/** Durable phantom allocation result. @since 1.6.0 @category models */
export interface ReflectedPhantomClient {
  readonly agentId: AgentIdentity.Identity
  readonly phantomId: string
  readonly client: ReflectedAgentClient
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

/** Durable reflected client factory. @since 1.6.0 @category models */
export interface DurableClientFactory {
  readonly get: (
    input: JsonValue,
    config?: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getValue: (
    input: CoreTypes.SchemaValueTree,
    config?: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getPhantom: (
    input: JsonValue,
    phantomId: string,
    config?: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getPhantomValue: (
    input: CoreTypes.SchemaValueTree,
    phantomId: string,
    config?: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly newPhantom: (
    input: JsonValue,
    config?: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<
    ReflectedPhantomClient,
    ReflectionError,
    AgentHostClient | DurabilityModeClient | RpcClient | Scope.Scope
  >
  readonly newPhantomValue: (
    input: CoreTypes.SchemaValueTree,
    config?: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<
    ReflectedPhantomClient,
    ReflectionError,
    AgentHostClient | DurabilityModeClient | RpcClient | Scope.Scope
  >
}

/** Ephemeral factory; each invocation allocates its own identity. @since 1.6.0 @category models */
export interface EphemeralClientFactory {
  readonly getPhantom: (
    input: JsonValue,
    phantomId: string,
    config?: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly getPhantomValue: (
    input: CoreTypes.SchemaValueTree,
    phantomId: string,
    config?: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly newPhantom: (
    input: JsonValue,
    config?: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly newPhantomValue: (
    input: CoreTypes.SchemaValueTree,
    config?: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
}

interface AgentTypeMetadata extends IdentityBinding<
  ReflectedAgentClient,
  ReflectionError | ClientBindingError,
  RpcClient | Scope.Scope
> {
  readonly name: string
  readonly description: string
  readonly sourceLanguage: string
  readonly implementedBy: CoreTypes.ComponentId
  readonly constructorInput: SchemaRef
  readonly methods: ReadonlyArray<AgentMethod>
  readonly config: ReadonlyArray<ReflectedConfigDeclaration>
  readonly method: (name: string) => AgentMethod | undefined
  readonly bindWithConfig: (
    identity: AgentIdentity.Identity,
    config: ReadonlyArray<ReflectedConfigValueEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError | ClientBindingError,
    RpcClient | Scope.Scope
  >
  readonly bindWithJsonConfig: (
    identity: AgentIdentity.Identity,
    config: ReadonlyArray<ReflectedConfigJsonEntry>,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError | ClientBindingError,
    RpcClient | Scope.Scope
  >
}

/** Immutable reflected registration, narrowed by lifecycle mode. @since 1.6.0 @category models */
export type AgentType = AgentTypeMetadata &
  (
    | {
        readonly mode: "durable"
        readonly client: DurableClientFactory
        readonly agentId: (
          input: JsonValue,
          phantomId?: string,
        ) => Effect.Effect<AgentIdentity.Identity, ReflectionError, AgentHostClient>
        readonly agentIdValue: (
          input: CoreTypes.SchemaValueTree,
          phantomId?: string,
        ) => Effect.Effect<AgentIdentity.Identity, ReflectionError, AgentHostClient>
      }
    | {
        readonly mode: "ephemeral"
        readonly client: EphemeralClientFactory
        readonly agentId: (
          input: JsonValue,
          phantomId: string,
        ) => Effect.Effect<AgentIdentity.Identity, ReflectionError, AgentHostClient>
        readonly agentIdValue: (
          input: CoreTypes.SchemaValueTree,
          phantomId: string,
        ) => Effect.Effect<AgentIdentity.Identity, ReflectionError, AgentHostClient>
      }
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
const parsePhantom = (
  phantomId?: string,
): Effect.Effect<CoreTypes.Uuid | undefined, RemoteCallError> =>
  Effect.try({
    try: () => (phantomId === undefined ? undefined : CoreTypes.parseUuid(phantomId)),
    catch: (cause) => ({ _tag: "InvalidUuidError", value: phantomId!, reason: String(cause) }),
  })
const makeId = (typeName: string, input: CoreTypes.SchemaValueTree, phantomId?: string) =>
  AgentIdentity.make({
    typeName,
    constructorValue: input,
    ...(phantomId === undefined ? {} : { phantomId }),
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
  const agentIdValue = (input: CoreTypes.SchemaValueTree, phantomId?: string) =>
    Effect.flatMap(validate(constructorInput, input), () =>
      raw.mode === "ephemeral" && phantomId === undefined
        ? Effect.fail(
            new AgentIdentity.AgentIdentityError(
              new TypeError(`ephemeral agent type '${raw.typeName}' requires a phantom ID`),
            ),
          )
        : makeId(raw.typeName, input, phantomId),
    )
  const bind = (
    input: CoreTypes.SchemaValueTree,
    phantomId?: string,
    parsedPhantom?: CoreTypes.Uuid,
    configEntries: ReadonlyArray<AgentCommon.TypedAgentConfigValue> = [],
  ) =>
    Effect.gen(function* () {
      yield* validate(constructorInput, input)
      const uuid = parsedPhantom ?? (yield* parsePhantom(phantomId))
      const host = yield* RpcClient
      const rpc = yield* Effect.acquireRelease(
        host
          .connect(raw.typeName, input, uuid, configEntries)
          .pipe(Effect.mapError((error) => wrapHostThrow(error.cause))),
        (connection) => Effect.sync(() => connection.drop()),
      )
      return reflectedClient(result, rpc)
    })
  const newPhantomValue: DurableClientFactory["newPhantomValue"] = (input, entries = []) =>
    Effect.gen(function* () {
      const configEntries = yield* validateConfigValues(entries)
      const durability = yield* DurabilityModeClient
      const uuid = yield* Effect.try({
        try: () => durability.generateIdempotencyKey(),
        catch: wrapHostThrow,
      })
      const phantomId = uuidToString(uuid)
      const agentId = yield* agentIdValue(input, phantomId)
      const client = yield* bind(input, phantomId, undefined, configEntries)
      return Object.freeze({ agentId, phantomId, client })
    })
  const factoryBase = {
    newPhantom: (input: JsonValue, entries: ReadonlyArray<ReflectedConfigJsonEntry> = []) =>
      Effect.flatMap(pack(constructorInput, input), (value) =>
        Effect.flatMap(packConfigJson(entries), (configEntries) =>
          Effect.gen(function* () {
            const durability = yield* DurabilityModeClient
            const uuid = yield* Effect.try({
              try: () => durability.generateIdempotencyKey(),
              catch: wrapHostThrow,
            })
            const phantomId = uuidToString(uuid)
            const agentId = yield* agentIdValue(value, phantomId)
            const client = yield* bind(value, phantomId, undefined, configEntries)
            return Object.freeze({ agentId, phantomId, client })
          }),
        ),
      ),
    newPhantomValue,
  }
  const lifecycle =
    raw.mode === "ephemeral"
      ? {
          mode: "ephemeral" as const,
          client: Object.freeze({
            getPhantom: (
              input: JsonValue,
              id: string,
              entries: ReadonlyArray<ReflectedConfigJsonEntry> = [],
            ) =>
              Effect.flatMap(pack(constructorInput, input), (value) =>
                Effect.flatMap(packConfigJson(entries), (configEntries) =>
                  bind(value, id, undefined, configEntries),
                ),
              ),
            getPhantomValue: (
              input: CoreTypes.SchemaValueTree,
              id: string,
              entries: ReadonlyArray<ReflectedConfigValueEntry> = [],
            ) =>
              Effect.flatMap(validateConfigValues(entries), (configEntries) =>
                bind(input, id, undefined, configEntries),
              ),
            newPhantom: (input: JsonValue, entries: ReadonlyArray<ReflectedConfigJsonEntry> = []) =>
              Effect.flatMap(pack(constructorInput, input), (value) =>
                Effect.flatMap(packConfigJson(entries), (configEntries) =>
                  bind(value, undefined, undefined, configEntries),
                ),
              ),
            newPhantomValue: (
              input: CoreTypes.SchemaValueTree,
              entries: ReadonlyArray<ReflectedConfigValueEntry> = [],
            ) =>
              Effect.flatMap(validateConfigValues(entries), (configEntries) =>
                bind(input, undefined, undefined, configEntries),
              ),
          }) satisfies EphemeralClientFactory,
        }
      : {
          mode: "durable" as const,
          client: Object.freeze({
            ...factoryBase,
            get: (input, entries = []) =>
              Effect.flatMap(pack(constructorInput, input), (value) =>
                Effect.flatMap(packConfigJson(entries), (configEntries) =>
                  bind(value, undefined, undefined, configEntries),
                ),
              ),
            getValue: (input, entries = []) =>
              Effect.flatMap(validateConfigValues(entries), (configEntries) =>
                bind(input, undefined, undefined, configEntries),
              ),
            getPhantom: (input, id, entries = []) =>
              Effect.flatMap(pack(constructorInput, input), (value) =>
                Effect.flatMap(packConfigJson(entries), (configEntries) =>
                  bind(value, id, undefined, configEntries),
                ),
              ),
            getPhantomValue: (input, id, entries = []) =>
              Effect.flatMap(validateConfigValues(entries), (configEntries) =>
                bind(input, id, undefined, configEntries),
              ),
          } satisfies DurableClientFactory),
        }
  const bindExisting = (
    identity: AgentIdentity.Identity,
    configEntries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) =>
    Effect.gen(function* () {
      if (raw.mode === "ephemeral")
        return yield* Effect.fail(
          new ClientBindingError(
            `Cannot bind existing identity '${identity.encoded}' to ephemeral agent type '${raw.typeName}'; use getPhantom(...) or newPhantom(...)`,
          ),
        )
      if (identity.typeName !== raw.typeName)
        return yield* Effect.fail(
          new ClientBindingError(
            `Reflected agent type '${raw.typeName}' cannot bind '${identity.typeName}'`,
          ),
        )
      const checked = constructorInput.validateValue(identity.constructorValue)
      if (!checked.success)
        return yield* Effect.fail(
          new ClientBindingError(
            `Reflected agent type '${raw.typeName}' cannot bind identity '${identity.encoded}': constructor value does not conform to the reflected schema`,
          ),
        )
      const phantom = yield* Effect.try({
        try: () => AgentIdentity.rawPhantomId(identity),
        catch: (cause) => new ClientBindingError(String(cause)),
      })
      return yield* bind(identity.constructorValue, undefined, phantom, configEntries)
    })
  const result: AgentType = Object.freeze({
    name: raw.typeName,
    description: raw.description,
    sourceLanguage: raw.sourceLanguage,
    ...lifecycle,
    implementedBy: deepFreeze(structuredClone(registration.implementedBy)),
    constructorInput,
    methods,
    config,
    method: (name: string) => methods.find((method) => method.name === name),
    bindWithConfig: (
      identity: AgentIdentity.Identity,
      entries: ReadonlyArray<ReflectedConfigValueEntry>,
    ) =>
      Effect.flatMap(validateConfigValues(entries), (configEntries) =>
        bindExisting(identity, configEntries),
      ),
    bindWithJsonConfig: (
      identity: AgentIdentity.Identity,
      entries: ReadonlyArray<ReflectedConfigJsonEntry>,
    ) =>
      Effect.flatMap(packConfigJson(entries), (configEntries) =>
        bindExisting(identity, configEntries),
      ),
    agentId: (input: JsonValue, phantomId?: string) =>
      Effect.flatMap(pack(constructorInput, input), (value) => agentIdValue(value, phantomId)),
    agentIdValue,
    [bindIdentity]: (identity: AgentIdentity.Identity) => bindExisting(identity, []),
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
      const invokeValue = (input: CoreTypes.SchemaValueTree) =>
        validate(definition.input, input).pipe(
          Effect.flatMap(() => method.invoke(input)),
          Effect.flatMap((invocation) => {
            if (definition.output === undefined)
              return invocation.value === undefined
                ? Effect.succeed(invocation)
                : Effect.fail(new RemoteOutputError(`${name}: expected unit output`))
            if (invocation.value === undefined)
              return Effect.fail(new RemoteOutputError(`${name}: expected an output value`))
            const checked = definition.output.validateValue(invocation.value)
            return checked.success
              ? Effect.succeed(invocation)
              : Effect.fail(
                  new RemoteOutputError(
                    `${name}: ${checked.issues.map((issue) => issue.message).join("; ")}`,
                  ),
                )
          }),
        )
      const reflected: ReflectedMethod = Object.freeze({
        definition,
        invokeValue,
        invoke: (input: JsonValue) =>
          pack(definition.input, input).pipe(
            Effect.flatMap(invokeValue),
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
          validate(definition.input, input).pipe(Effect.flatMap(() => method.trigger(input))),
        trigger: (input: JsonValue) =>
          pack(definition.input, input).pipe(Effect.flatMap((value) => method.trigger(value))),
        scheduleValue: (at: AgentHost.Datetime, input: CoreTypes.SchemaValueTree) =>
          validate(definition.input, input).pipe(Effect.flatMap(() => method.schedule(at, input))),
        schedule: (at: AgentHost.Datetime, input: JsonValue) =>
          pack(definition.input, input).pipe(Effect.flatMap((value) => method.schedule(at, value))),
      })
      return Effect.succeed(reflected)
    },
  })

const deepFreeze = <T>(value: T): T => {
  if (value !== null && typeof value === "object") {
    for (const child of Object.values(value)) deepFreeze(child)
    Object.freeze(value)
  }
  return value
}

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

/** Optionally discover the current deployed type for an environment-scoped identity. @since 1.6.0 @category discovery */
export const getAgentTypeByAgentId = (
  identity: AgentIdentity.Identity,
): Effect.Effect<
  AgentType | undefined,
  ReflectionHostError | ReflectionSchemaError,
  AgentHostClient
> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    const found = yield* Effect.try({
      try: () => host.getAgentTypeByAgentId(identity.encoded),
      catch: (cause) => new ReflectionHostError(cause),
    })
    if (found === undefined) return undefined
    return yield* Effect.try({
      try: () => wrap(found),
      catch: (cause) => new ReflectionSchemaError(cause),
    })
  })
