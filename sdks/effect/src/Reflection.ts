/** Effect-native runtime agent reflection and schema-checked clients. @since 1.6.0 */
import { Effect, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import * as CoreTypes from "golem:core/types@2.0.0"
import { uuidToString } from "golem:core/types@2.0.0"
import type { RemoteCallError } from "./Client.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RpcClient, type RpcConnection } from "./host/RpcClient.js"
import { dynamicMethod } from "./internal/dynamicMethod.js"
import { wrapHostThrow } from "./internal/rpc.js"
import { SchemaRef, SchemaRenderError, type JsonValue } from "./SchemaRef.js"

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
  | AgentIdentityError

/** A method name absent from the reflected type. @since 1.6.0 @category errors */
export class UnknownMethodError {
  readonly _tag = "UnknownMethodError"
  constructor(
    readonly agentType: string,
    readonly method: string,
  ) {}
}

/** The host rejected construction of an agent identity. @since 1.6.0 @category errors */
export class AgentIdentityError {
  readonly _tag = "AgentIdentityError"
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
  readonly agentId: string
  readonly phantomId: string
  readonly client: ReflectedAgentClient
}

/** Durable reflected client factory. @since 1.6.0 @category models */
export interface DurableClientFactory {
  readonly get: (
    input: JsonValue,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getPhantom: (
    input: JsonValue,
    phantomId: string,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly getPhantomValue: (
    input: CoreTypes.SchemaValueTree,
    phantomId: string,
  ) => Effect.Effect<
    ReflectedAgentClient,
    ReflectionError,
    AgentHostClient | RpcClient | Scope.Scope
  >
  readonly newPhantom: (
    input: JsonValue,
  ) => Effect.Effect<
    ReflectedPhantomClient,
    ReflectionError,
    AgentHostClient | DurabilityModeClient | RpcClient | Scope.Scope
  >
  readonly newPhantomValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<
    ReflectedPhantomClient,
    ReflectionError,
    AgentHostClient | DurabilityModeClient | RpcClient | Scope.Scope
  >
}

/** Ephemeral factory; each invocation allocates its own identity. @since 1.6.0 @category models */
export interface EphemeralClientFactory {
  readonly newPhantom: (
    input: JsonValue,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
  readonly newPhantomValue: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<ReflectedAgentClient, ReflectionError, RpcClient | Scope.Scope>
}

interface AgentTypeMetadata {
  readonly name: string
  readonly description: string
  readonly sourceLanguage: string
  readonly implementedBy: CoreTypes.ComponentId
  readonly constructorInput: SchemaRef
  readonly methods: ReadonlyArray<AgentMethod>
  readonly method: (name: string) => AgentMethod | undefined
  readonly agentId: (
    input: JsonValue,
    phantomId?: string,
  ) => Effect.Effect<string, ReflectionError, AgentHostClient>
  readonly agentIdValue: (
    input: CoreTypes.SchemaValueTree,
    phantomId?: string,
  ) => Effect.Effect<string, ReflectionError, AgentHostClient>
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
const parsePhantom = (
  phantomId?: string,
): Effect.Effect<CoreTypes.Uuid | undefined, RemoteCallError> =>
  Effect.try({
    try: () => (phantomId === undefined ? undefined : CoreTypes.parseUuid(phantomId)),
    catch: (cause) => ({ _tag: "InvalidUuidError", value: phantomId!, reason: String(cause) }),
  })
const makeId = (typeName: string, input: CoreTypes.SchemaValueTree, phantomId?: string) =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    const uuid = yield* parsePhantom(phantomId)
    return yield* Effect.try({
      try: () => host.makeAgentId(typeName, input, uuid),
      catch: (cause) => new AgentIdentityError(cause),
    })
  })

const wrap = (registration: AgentHost.RegisteredAgentType): AgentType => {
  const raw = structuredClone(registration.agentType)
  const inputRef = (input: AgentCommon.InputSchema) => {
    const fields = input.val.filter((field) => field.source.tag === "user-supplied")
    const graph = structuredClone(raw.schema)
    graph.typeNodes.push({
      body: {
        tag: "record-type",
        val: fields.map((field) => ({
          name: field.name,
          body: field.schema,
          metadata: field.metadata,
        })),
      },
      metadata: { aliases: [], examples: [] },
    })
    return new SchemaRef(graph, graph.typeNodes.length - 1)
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
            : { output: new SchemaRef(raw.schema, method.outputSchema.val) }),
        }),
    ),
  )
  const constructorInput = inputRef(raw.constructor.inputSchema)
  const agentIdValue = (input: CoreTypes.SchemaValueTree, phantomId?: string) =>
    Effect.flatMap(validate(constructorInput, input), () => makeId(raw.typeName, input, phantomId))
  const bind = (input: CoreTypes.SchemaValueTree, phantomId?: string) =>
    Effect.gen(function* () {
      yield* validate(constructorInput, input)
      const uuid = yield* parsePhantom(phantomId)
      const host = yield* RpcClient
      const rpc = yield* Effect.acquireRelease(
        host
          .connect(raw.typeName, input, uuid, [])
          .pipe(Effect.mapError((error) => wrapHostThrow(error.cause))),
        (connection) => Effect.sync(() => connection.drop()),
      )
      return reflectedClient(result, rpc)
    })
  const newPhantomValue: DurableClientFactory["newPhantomValue"] = (input) =>
    Effect.gen(function* () {
      const durability = yield* DurabilityModeClient
      const uuid = yield* Effect.try({
        try: () => durability.generateIdempotencyKey(),
        catch: wrapHostThrow,
      })
      const phantomId = uuidToString(uuid)
      const agentId = yield* agentIdValue(input, phantomId)
      const client = yield* bind(input, phantomId)
      return Object.freeze({ agentId, phantomId, client })
    })
  const factoryBase = {
    newPhantom: (input: JsonValue) =>
      Effect.flatMap(pack(constructorInput, input), newPhantomValue),
    newPhantomValue,
  }
  const lifecycle =
    raw.mode === "ephemeral"
      ? {
          mode: "ephemeral" as const,
          client: Object.freeze({
            newPhantom: (input: JsonValue) =>
              Effect.flatMap(pack(constructorInput, input), (value) => bind(value)),
            newPhantomValue: (input: CoreTypes.SchemaValueTree) => bind(input),
          }) satisfies EphemeralClientFactory,
        }
      : {
          mode: "durable" as const,
          client: Object.freeze({
            ...factoryBase,
            get: (input) => Effect.flatMap(pack(constructorInput, input), (value) => bind(value)),
            getValue: (input) => bind(input),
            getPhantom: (input, id) =>
              Effect.flatMap(pack(constructorInput, input), (value) => bind(value, id)),
            getPhantomValue: (input, id) => bind(input, id),
          } satisfies DurableClientFactory),
        }
  const result: AgentType = Object.freeze({
    name: raw.typeName,
    description: raw.description,
    sourceLanguage: raw.sourceLanguage,
    ...lifecycle,
    implementedBy: deepFreeze(structuredClone(registration.implementedBy)),
    constructorInput,
    methods,
    method: (name: string) => methods.find((method) => method.name === name),
    agentId: (input: JsonValue, phantomId?: string) =>
      Effect.flatMap(pack(constructorInput, input), (value) => agentIdValue(value, phantomId)),
    agentIdValue,
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
  never,
  AgentHostClient
> = Effect.map(AgentHostClient, (host) => Object.freeze(host.getAllAgentTypes().map(wrap)))

/** Optionally discover a deployed agent type by name. @since 1.6.0 @category discovery */
export const getAgentType = (
  name: string,
): Effect.Effect<AgentType | undefined, never, AgentHostClient> =>
  Effect.map(AgentHostClient, (host) => {
    const found = host.getAgentType(name)
    return found === undefined ? undefined : wrap(found)
  })

/** Optionally discover the current deployed type for an environment-scoped identity. @since 1.6.0 @category discovery */
export const getAgentTypeByAgentId = (
  agentId: string,
): Effect.Effect<AgentType | undefined, never, AgentHostClient> =>
  Effect.map(AgentHostClient, (host) => {
    const found = host.getAgentTypeByAgentId(agentId)
    return found === undefined ? undefined : wrap(found)
  })
