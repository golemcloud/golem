/** Parsed, environment-scoped agent identities. @since 1.6.0 */
import { Effect } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { bind, type IdentityBinding } from "./Client.js"
import { bind as bindDynamic } from "./DynamicClient.js"

/** Identity parsing or construction failed at the host boundary. @since 1.6.0 @category errors */
export class AgentIdentityError {
  readonly _tag = "AgentIdentityError"
  constructor(readonly cause: unknown) {}
}

/** Immutable semantic parts of an environment-scoped agent identity. @since 1.6.0 @category models */
export interface Identity {
  readonly encoded: string
  readonly typeName: string
  readonly constructorValue: CoreTypes.SchemaValueTree
  readonly phantomId?: string
  /** Bind a method-only, full, or reflected client without rediscovery. @since 1.6.0 @category constructors */
  readonly client: <Client, Error, Requirements>(
    binding: IdentityBinding<Client, Error, Requirements>,
  ) => Effect.Effect<Client, Error, Requirements>
  /** Bind schema-value methods without a typed client definition or discovery. @since 1.6.0 @category constructors */
  readonly dynamicClient: () => ReturnType<typeof bindDynamic>
}

/** Input used to construct an environment-scoped identity. @since 1.6.0 @category models */
export interface MakeOptions {
  readonly typeName: string
  readonly constructorValue: CoreTypes.SchemaValueTree
  readonly phantomId?: string
}

const rawPhantoms = new WeakMap<Identity, CoreTypes.Uuid | undefined>()

const copyValue = <T>(value: T): T => {
  if (value instanceof Uint8Array) return value.slice() as T
  if (Array.isArray(value)) return value.map(copyValue) as T
  if (value !== null && typeof value === "object") {
    const prototype = Object.getPrototypeOf(value)
    if (prototype === Object.prototype || prototype === null)
      return Object.fromEntries(
        Object.entries(value).map(([key, item]) => [key, copyValue(item)]),
      ) as T
  }
  // Native capability handles are opaque and cannot be cloned or frozen.
  return value
}

const deepFreeze = <T>(value: T, seen = new WeakSet<object>()): T => {
  if (value === null || typeof value !== "object" || seen.has(value)) return value
  if (value instanceof Uint8Array) return value
  const prototype = Object.getPrototypeOf(value)
  if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null) return value
  seen.add(value)
  for (const item of Object.values(value)) deepFreeze(item, seen)
  return Object.freeze(value)
}

const create = (
  encoded: string,
  typeName: string,
  constructorValue: CoreTypes.SchemaValueTree,
  phantom: CoreTypes.Uuid | undefined,
): Identity => {
  const immutableConstructor = copyValue(constructorValue)
  const identity = Object.freeze({
    encoded,
    typeName,
    get constructorValue() {
      return deepFreeze(copyValue(immutableConstructor))
    },
    client: <Client, Error, Requirements>(
      binding: IdentityBinding<Client, Error, Requirements>,
    ): Effect.Effect<Client, Error, Requirements> => bind(identity, binding),
    dynamicClient: () => bindDynamic(identity),
    ...(phantom === undefined ? {} : { phantomId: uuidToString(phantom) }),
  })
  rawPhantoms.set(identity, phantom)
  return identity
}

/** Strictly parse an encoded identity once and retain its immutable semantic parts. @since 1.6.0 @category constructors */
export const parse = (
  encoded: string,
): Effect.Effect<Identity, AgentIdentityError, AgentHostClient> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    return yield* Effect.try({
      try: () => {
        const [typeName, constructor, phantom] = host.parseAgentId(encoded)
        return create(encoded, typeName, constructor.value, phantom)
      },
      catch: (cause) => new AgentIdentityError(cause),
    })
  })

/** Construct an encoded identity from schema-native constructor data. @since 1.6.0 @category constructors */
export const make = (
  options: MakeOptions,
): Effect.Effect<Identity, AgentIdentityError, AgentHostClient> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    return yield* Effect.try({
      try: () => {
        const phantom = options.phantomId === undefined ? undefined : parseUuid(options.phantomId)
        const encoded = host.makeAgentId(options.typeName, options.constructorValue, phantom)
        return create(encoded, options.typeName, options.constructorValue, phantom)
      },
      catch: (cause) => new AgentIdentityError(cause),
    })
  })

/** @internal Raw UUID retained by parsing/construction so clients never reparse the identity string. */
export const rawPhantomId = (identity: Identity): CoreTypes.Uuid | undefined => {
  if (!rawPhantoms.has(identity)) {
    throw new TypeError(
      "Expected an AgentIdentity created by AgentIdentity.parse or AgentIdentity.make",
    )
  }
  return rawPhantoms.get(identity)
}
