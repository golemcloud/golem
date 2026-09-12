/** Effect-native runtime used by generated guest agent bridges. @since 1.6.0 */
import { Effect, Schema, SchemaGetter, Scope } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import type { RemoteCallError as ClientRemoteCallError } from "./Client.js"
import * as Datetime from "./Datetime.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { RpcClient, type RpcConnection } from "./host/RpcClient.js"
import { AgentStream } from "./internal/agentStream.js"
import {
  agentStreamFromHandle as fromStreamHandle,
  agentStreamToHandle as toStreamHandle,
} from "./internal/agentStream.js"
import { awaitInvocation, scheduleCancelableInvocation, wrapHostThrow } from "./internal/rpc.js"
import { withCapabilityAdoptionTransaction } from "./internal/schema-model/capabilityTransaction.js"
import type { GuestPermissionCardHandle } from "./internal/schema-model/permissionCardHandle.js"
import type { GuestSecretHandle } from "./internal/schema-model/secretHandle.js"
import type { SchemaGraph, SchemaValue, TypedSchemaValue } from "./internal/schema-model/model.js"
import {
  schemaValueFromWit,
  schemaValueToWitAsync,
  typedSchemaValueToWit,
} from "./internal/schema-model/wit.js"
import { QuotaTokenSchema, type QuotaToken } from "./Quota.js"
import {
  unstructuredBinaryFromValue,
  unstructuredBinaryToValue,
  unstructuredTextFromValue,
  unstructuredTextToValue,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "./Unstructured.js"

/** Re-exported schema model used by generated bridge source. @since 1.6.0 @category models */
export * from "./internal/schema-model/model.js"

/** Structural codec emitted for each generated source schema. @since 1.6.0 @category codecs */
export interface SchemaCodec<T = any> {
  readonly graph: SchemaGraph
  readonly toValue: (value: T) => SchemaValue
  readonly fromValue: (value: SchemaValue) => T
}

/** Result representation used by generated bridge methods. @since 1.6.0 @category models */
export type JsonResult<A, E> = { readonly ok: A } | { readonly err: E }
/** Secret capability used in generated schemas. @since 1.6.0 @category capabilities */
export type SecretHandle = GuestSecretHandle
/** Permission-card capability used in generated schemas. @since 1.6.0 @category capabilities */
export type PermissionCardHandle = GuestPermissionCardHandle
/** Quota capability used in generated schemas. @since 1.6.0 @category capabilities */
export type { QuotaToken }
/** Native stream used in generated schemas. @since 1.6.0 @category streams */
export { AgentStream }
/** Schema text payload alias emitted by the generator. @since 1.6.0 @category models */
export type AgentText = string
/** Schema binary payload alias emitted by the generator. @since 1.6.0 @category models */
export type AgentBinary = Uint8Array
/** Schema quantity payload alias emitted by the generator. @since 1.6.0 @category models */
export type QuantityValue = CoreTypes.QuantityValue
/** Typed failure from bridge connection and invocation operations. @since 1.6.0 @category errors */
export type RemoteCallError = ClientRemoteCallError
/** Typed failure converting a scheduled invocation time. @since 1.6.0 @category errors */
export type DatetimeConversionError = Datetime.DatetimeConversionError

/** Plain generated-code wrapper for unstructured text references. @since 1.6.0 @category models */
export class UnstructuredText {
  private constructor(readonly value: TextReferenceValue) {}
  /** Reference discriminator. @since 1.6.0 @category models */
  get tag() {
    return this.value._tag
  }
  /** Reference payload. @since 1.6.0 @category models */
  get val() {
    return this.value.val
  }
  /** Inline language restriction. @since 1.6.0 @category models */
  get languageCode() {
    return this.value._tag === "inline" ? this.value.languageCode : undefined
  }
  /** Construct an inline text reference. @since 1.6.0 @category constructors */
  static fromInline(text: string, languageCode?: string): UnstructuredText {
    return new UnstructuredText({ _tag: "inline", val: text, languageCode })
  }
  /** Construct a URL text reference. @since 1.6.0 @category constructors */
  static fromUrl(url: string): UnstructuredText {
    return new UnstructuredText({ _tag: "url", val: url })
  }
  /** Decode a schema value. @since 1.6.0 @category conversions */
  static fromSchemaValue(
    context: string,
    value: SchemaValue,
    languages: readonly string[],
  ): UnstructuredText {
    return new UnstructuredText(unstructuredTextFromValue(context, value, languages))
  }
  /** Encode this reference as a schema value. @since 1.6.0 @category conversions */
  static toSchemaValue(value: UnstructuredText): SchemaValue {
    return unstructuredTextToValue(value.value)
  }
}
/** Generated unstructured text type. @since 1.6.0 @category models */
export type UnstructuredTextType<LC extends readonly string[] = readonly string[]> =
  UnstructuredText & { readonly __languages?: LC }

/** Plain generated-code wrapper for unstructured binary references. @since 1.6.0 @category models */
export class UnstructuredBinary {
  private constructor(readonly value: BinaryReferenceValue) {}
  /** Reference discriminator. @since 1.6.0 @category models */
  get tag() {
    return this.value._tag
  }
  /** Reference payload. @since 1.6.0 @category models */
  get val() {
    return this.value.val
  }
  /** Inline MIME restriction. @since 1.6.0 @category models */
  get mimeType() {
    return this.value._tag === "inline" ? this.value.mimeType : undefined
  }
  /** Construct an inline binary reference. @since 1.6.0 @category constructors */
  static fromInline(bytes: Uint8Array, mimeType?: string): UnstructuredBinary {
    return new UnstructuredBinary({ _tag: "inline", val: bytes, mimeType })
  }
  /** Construct a URL binary reference. @since 1.6.0 @category constructors */
  static fromUrl(url: string): UnstructuredBinary {
    return new UnstructuredBinary({ _tag: "url", val: url })
  }
  /** Decode a schema value. @since 1.6.0 @category conversions */
  static fromSchemaValue(
    context: string,
    value: SchemaValue,
    mimeTypes: readonly string[],
  ): UnstructuredBinary {
    return new UnstructuredBinary(unstructuredBinaryFromValue(context, value, mimeTypes))
  }
  /** Encode this reference as a schema value. @since 1.6.0 @category conversions */
  static toSchemaValue(value: UnstructuredBinary): SchemaValue {
    return unstructuredBinaryToValue(value.value)
  }
}
/** Generated unstructured binary type. @since 1.6.0 @category models */
export type UnstructuredBinaryType<MT extends readonly string[] | string = string> =
  UnstructuredBinary & { readonly __mimeTypes?: MT }

const effectCodec = <T>(codec: SchemaCodec<T>): Schema.Codec<T, SchemaValue> =>
  Schema.declare((_): _ is SchemaValue => true).pipe(
    Schema.decodeTo(
      Schema.declare((_): _ is T => true),
      {
        decode: SchemaGetter.transform(codec.fromValue),
        encode: SchemaGetter.transform(codec.toValue),
      },
    ),
  )

/** Transfer a native stream into a generated schema value. @since 1.6.0 @category streams */
export const agentStreamToHandle = <T>(stream: AgentStream<T>, codec: SchemaCodec<T>) =>
  toStreamHandle(stream, effectCodec(codec))
/** Lift a generated schema stream while retaining its source item codec. @since 1.6.0 @category streams */
export const agentStreamFromHandle = <T>(
  handle: Parameters<typeof fromStreamHandle>[0],
  codec: SchemaCodec<T>,
): AgentStream<T> => fromStreamHandle(handle, effectCodec(codec))

/** Adopt capabilities atomically while constructing a schema value. @since 1.6.0 @category conversions */
export { withCapabilityAdoptionTransaction }
/** Encode an optional generated value. @since 1.6.0 @category conversions */
export const encodeOption = <T>(
  value: T | undefined,
  encode: (value: T) => SchemaValue,
): SchemaValue => ({ tag: "option", value: value === undefined ? undefined : encode(value) })
/** Decode an optional generated value. @since 1.6.0 @category conversions */
export const decodeOption = <T>(
  value: SchemaValue,
  decode: (value: SchemaValue) => T,
): T | undefined => {
  if (value.tag !== "option") throw new Error(`Expected option, received ${value.tag}`)
  return value.value === undefined ? undefined : decode(value.value)
}
/** Encode named flags in wire order. @since 1.6.0 @category conversions */
export const encodeFlags = (
  value: Record<string, boolean>,
  names: readonly (readonly [string, string])[],
): SchemaValue => ({ tag: "flags", flags: names.map(([property]) => value[property] ?? false) })
/** Decode named flags from wire order. @since 1.6.0 @category conversions */
export const decodeFlags = <T extends Record<string, boolean>>(
  value: SchemaValue,
  target: T,
  names: readonly (readonly [string, string])[],
): T => {
  if (value.tag !== "flags" || value.flags.length !== names.length)
    throw new Error("Invalid flags schema value")
  names.forEach(([property], index) => {
    ;(target as Record<string, boolean>)[property] = value.flags[index]!
  })
  return target
}

/** Convert a UTC ISO instant to a schema datetime. @since 1.6.0 @category conversions */
export const datetimeFromISOString = (value: string): CoreTypes.Datetime => {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/.exec(value)
  if (!match) throw new Error(`Invalid UTC datetime '${value}'`)
  const whole = `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}`
  const milliseconds = Date.parse(`${whole}Z`)
  if (!Number.isFinite(milliseconds) || new Date(milliseconds).toISOString().slice(0, 19) !== whole)
    throw new Error(`Invalid datetime '${value}'`)
  return {
    seconds: BigInt(milliseconds / 1000),
    nanoseconds: Number((match[7] ?? "").padEnd(9, "0") || 0),
  }
}
/** Convert a schema datetime to canonical UTC ISO. @since 1.6.0 @category conversions */
export const datetimeToISOString = (value: CoreTypes.Datetime): string => {
  if (
    !Number.isInteger(value.nanoseconds) ||
    value.nanoseconds < 0 ||
    value.nanoseconds >= 1_000_000_000
  )
    throw new Error("Datetime nanoseconds out of range")
  const base = new Date(Number(value.seconds * 1000n)).toISOString().replace(".000Z", "")
  const fraction =
    value.nanoseconds === 0
      ? ""
      : `.${String(value.nanoseconds).padStart(9, "0").replace(/0+$/, "")}`
  return `${base}${fraction}Z`
}

/** Encode an Effect quota token as a schema capability. @since 1.6.0 @category conversions */
export const quotaTokenToSchemaValue = (value: QuotaToken): SchemaValue => ({
  tag: "quota-token",
  handle: Schema.encodeUnknownSync(QuotaTokenSchema)(value),
})
/** Decode an Effect quota token from a schema capability. @since 1.6.0 @category conversions */
export const quotaTokenFromSchemaValue = (value: SchemaValue): QuotaToken => {
  if (value.tag !== "quota-token") throw new Error(`Expected quota-token, received ${value.tag}`)
  return Schema.decodeUnknownSync(QuotaTokenSchema)(value.handle)
}
/** Encode a secret handle as a schema capability. @since 1.6.0 @category conversions */
export const secretHandleToSchemaValue = (handle: SecretHandle): SchemaValue => ({
  tag: "secret",
  handle,
})
/** Decode a secret handle from a schema capability. @since 1.6.0 @category conversions */
export const secretHandleFromSchemaValue = (value: SchemaValue): SecretHandle => {
  if (value.tag !== "secret") throw new Error(`Expected secret, received ${value.tag}`)
  return value.handle
}
/** Encode a permission card as a schema capability. @since 1.6.0 @category conversions */
export const permissionCardHandleToSchemaValue = (handle: PermissionCardHandle): SchemaValue => ({
  tag: "permission-card",
  handle,
})
/** Decode a permission card from a schema capability. @since 1.6.0 @category conversions */
export const permissionCardHandleFromSchemaValue = (value: SchemaValue): PermissionCardHandle => {
  if (value.tag !== "permission-card")
    throw new Error(`Expected permission-card, received ${value.tag}`)
  return value.handle
}

/** Config override shape consumed by generated constructors. @since 1.6.0 @category models */
export interface AgentConfigEntry {
  readonly path: readonly string[]
  readonly value: TypedSchemaValue
}
/** Awaited invocation result with host metadata. @since 1.6.0 @category models */
export interface RemoteInvocationResult<A> {
  readonly metadata: AgentHost.InvocationMetadata
  readonly value: A
}
/** Definition-independent generated guest RPC handle. @since 1.6.0 @category clients */
export interface RemoteAgentHandle {
  readonly agentId: string
  readonly invokeAndAwait: <A>(
    name: string,
    input: SchemaValue,
    decode: (value: SchemaValue | undefined) => A,
  ) => Effect.Effect<A, RemoteCallError>
  readonly invokeAndAwaitWithMetadata: <A>(
    name: string,
    input: SchemaValue,
    decode: (value: SchemaValue | undefined) => A,
  ) => Effect.Effect<RemoteInvocationResult<A>, RemoteCallError>
  readonly invoke: (name: string, input: SchemaValue) => Effect.Effect<void, RemoteCallError>
  readonly invokeWithMetadata: (
    name: string,
    input: SchemaValue,
  ) => Effect.Effect<AgentHost.InvocationMetadata, RemoteCallError>
  readonly scheduleCancelable: (
    at: Datetime.Input,
    name: string,
    input: SchemaValue,
  ) => Effect.Effect<
    { readonly cancel: Effect.Effect<void> },
    RemoteCallError | Datetime.DatetimeConversionError,
    Scope.Scope
  >
  readonly scheduleCancelableWithMetadata: (
    at: Datetime.Input,
    name: string,
    input: SchemaValue,
  ) => Effect.Effect<
    { readonly metadata: AgentHost.InvocationMetadata; readonly cancel: Effect.Effect<void> },
    RemoteCallError | Datetime.DatetimeConversionError,
    Scope.Scope
  >
}

/** Host services and scope needed by generated constructors. @since 1.6.0 @category models */
export type ConnectionRequirements = RpcClient | AgentHostClient | Scope.Scope
/** Host service needed for a fresh phantom identity. @since 1.6.0 @category models */
export type PhantomRequirements = DurabilityModeClient
/** Suspend generated codecs and report failures in the Effect error channel. @since 1.6.0 @category codecs */
export const attempt = <A>(body: () => A): Effect.Effect<A, RemoteCallError> =>
  Effect.try({ try: () => withCapabilityAdoptionTransaction(body), catch: wrapHostThrow })

const encode = (value: SchemaValue) =>
  Effect.tryPromise({ try: (signal) => schemaValueToWitAsync(value, signal), catch: wrapHostThrow })

/** Lift and decode a wire result under one capability transaction. @since 1.6.0 @category codecs */
export const decodeWire = <A>(
  value: CoreTypes.SchemaValueTree | undefined,
  decode: (value: SchemaValue | undefined) => A,
): A => {
  try {
    return withCapabilityAdoptionTransaction((transaction) =>
      decode(value === undefined ? undefined : schemaValueFromWit(value, transaction)),
    )
  } catch (error) {
    for (const node of value?.valueNodes ?? []) {
      if (node.tag === "stream-value" && node.val !== undefined) {
        const stream = node.val as { [Symbol.dispose]?: () => void }
        try {
          stream[Symbol.dispose]?.()
        } finally {
          node.val = undefined as never
        }
      }
    }
    throw error
  }
}

const remote = (rpc: RpcConnection, agentId: string): RemoteAgentHandle => {
  const awaitWithMetadata = <A>(
    name: string,
    input: SchemaValue,
    decode: (value: SchemaValue | undefined) => A,
  ) =>
    encode(input).pipe(
      Effect.flatMap((tree) => awaitInvocation(rpc, name, tree)),
      Effect.flatMap(({ metadata, result }) =>
        Effect.try({
          try: () => ({
            metadata,
            value: decodeWire(result, decode),
          }),
          catch: wrapHostThrow,
        }),
      ),
    )
  const invokeWithMetadata = (name: string, input: SchemaValue) =>
    encode(input).pipe(
      Effect.flatMap((tree) =>
        Effect.try({ try: () => rpc.invoke(name, tree), catch: wrapHostThrow }),
      ),
    )
  const scheduleWithMetadata = (at: Datetime.Input, name: string, input: SchemaValue) =>
    Datetime.fromInput(at).pipe(
      Effect.flatMap((time) => encode(input).pipe(Effect.map((tree) => [time, tree] as const))),
      Effect.flatMap(([time, tree]) => scheduleCancelableInvocation(rpc, time, name, tree)),
    )
  return {
    agentId,
    invokeAndAwait: (name, input, decode) =>
      Effect.map(awaitWithMetadata(name, input, decode), ({ value }) => value),
    invokeAndAwaitWithMetadata: awaitWithMetadata,
    invoke: (name, input) => Effect.asVoid(invokeWithMetadata(name, input)),
    invokeWithMetadata,
    scheduleCancelable: (at, name, input) =>
      Effect.map(scheduleWithMetadata(at, name, input), ({ cancel }) => ({ cancel })),
    scheduleCancelableWithMetadata: scheduleWithMetadata,
  }
}

/** Resolve a remote guest agent using injected host services and scoped ownership. @since 1.6.0 @category clients */
export const resolveRemoteAgent = (
  agentTypeName: string,
  constructor: SchemaValue,
  phantomId: string | undefined,
  configs: readonly AgentConfigEntry[],
  mode: "durable" | "ephemeral",
): Effect.Effect<RemoteAgentHandle, RemoteCallError, RpcClient | AgentHostClient | Scope.Scope> =>
  Effect.suspend(() =>
    Effect.gen(function* () {
      const constructorTree = yield* encode(constructor)
      const phantom =
        phantomId === undefined
          ? undefined
          : yield* Effect.try({ try: () => parseUuid(phantomId), catch: wrapHostThrow })
      const agentHost = yield* AgentHostClient
      const agentId =
        mode === "durable"
          ? yield* Effect.try({
              try: () => agentHost.makeAgentId(agentTypeName, constructorTree, phantom),
              catch: wrapHostThrow,
            })
          : agentTypeName
      const rpcHost = yield* RpcClient
      const wireConfigs = yield* attempt(() =>
        configs.map(({ path, value }) => ({
          path: [...path],
          value: typedSchemaValueToWit(value),
        })),
      )
      const rpc = yield* Effect.acquireRelease(
        Effect.mapError(
          rpcHost.connect(agentTypeName, constructorTree, phantom, wireConfigs),
          (error) => wrapHostThrow(error.cause),
        ),
        (connection) => Effect.sync(() => connection.drop()),
      )
      return remote(rpc, agentId)
    }),
  )

/** Generate a durable phantom identity from the host durability service. @since 1.6.0 @category clients */
export const generatePhantomId: Effect.Effect<string, RemoteCallError, DurabilityModeClient> =
  Effect.gen(function* () {
    const host = yield* DurabilityModeClient
    return yield* Effect.try({
      try: () => uuidToString(host.generateIdempotencyKey()),
      catch: wrapHostThrow,
    })
  })
