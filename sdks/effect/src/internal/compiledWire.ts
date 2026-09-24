import type {
  SchemaGraph,
  SchemaValueNode,
  SchemaValueStream,
  SchemaValueTree,
} from "golem:core/types@2.0.0"
import { Context, Effect, Exit, Schema, SchemaIssue } from "effect"
import { directAgentStreamFromHandle, directAgentStreamToHandle } from "./agentStream.js"
import { CapabilityTransaction } from "./schema-model/capabilityTransaction.js"
import { PreparedStream } from "./schema-model/preparedStream.js"
import {
  assertGuestSecretHandleCanLiftFromWire,
  liftGuestSecretHandleFromWire,
  peekGuestSecretHandle,
  takeGuestSecretHandleToWire,
  type GuestSecretHandle,
} from "./schema-model/secretHandle.js"
import { SECRET_INTERNAL } from "./schema-model/secretInternal.js"
import {
  abandonGuestQuotaTokenWireHandle,
  assertGuestQuotaTokenHandleCanLiftFromWire,
  liftGuestQuotaTokenHandleFromWire,
  peekGuestQuotaTokenHandle,
  takeGuestQuotaTokenHandleToWire,
  type GuestQuotaTokenHandle,
} from "./schema-model/quotaTokenHandle.js"
import { QUOTA_INTERNAL } from "./schema-model/quotaInternal.js"
import {
  abandonGuestPermissionCardWireHandle,
  assertGuestPermissionCardHandleCanLiftFromWire,
  liftGuestPermissionCardHandleFromWire,
  peekGuestPermissionCardHandle,
  takeGuestPermissionCardHandleToWire,
  type GuestPermissionCardHandle,
} from "./schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "./schema-model/permissionCardInternal.js"
import { GuestSchemaValueStreamHandle } from "./schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "./schema-model/streamInternal.js"

export interface ConcreteCodec {
  readonly schema?: Schema.Top
  read(reader: WireReader, index: number): unknown
  write(value: unknown, writer: WireWriter): number
}

type OwnedTag = "secret-value" | "quota-token-handle" | "permission-card-handle"
type OwnedHandle = GuestSecretHandle | GuestQuotaTokenHandle | GuestPermissionCardHandle

export class WireReader {
  private readonly visited = new Set<number>()
  private readonly raw = new Set<object>()

  constructor(
    readonly tree: SchemaValueTree,
    private readonly transaction: CapabilityTransaction,
    private readonly context: Context.Context<any> = Context.empty() as Context.Context<any>,
  ) {
    if (!Array.isArray(tree?.valueNodes)) throw new TypeError("invalid wire arena")
  }

  node<T>(index: number, tag: SchemaValueNode["tag"], decode: (node: any) => T): T {
    if (!Number.isInteger(index) || index < 0 || index >= this.tree.valueNodes.length)
      throw new TypeError(`invalid wire index ${index}`)
    if (this.visited.has(index)) throw new TypeError(`aliased or cyclic wire index ${index}`)
    const node = this.tree.valueNodes[index]!
    if (node.tag !== tag) throw new TypeError(`expected ${tag}, received ${node.tag}`)
    this.visited.add(index)
    return decode(node)
  }

  resource(node: { tag: OwnedTag; val: unknown }): OwnedHandle {
    const raw = node.val
    if (raw === null || typeof raw !== "object" || this.raw.has(raw))
      throw new TypeError("invalid or aliased resource")
    this.raw.add(raw)
    if (node.tag === "secret-value")
      assertGuestSecretHandleCanLiftFromWire(SECRET_INTERNAL, raw as any, node)
    else if (node.tag === "quota-token-handle")
      assertGuestQuotaTokenHandleCanLiftFromWire(QUOTA_INTERNAL, raw as any, node)
    else assertGuestPermissionCardHandleCanLiftFromWire(PERMISSION_CARD_INTERNAL, raw as any, node)
    node.val = undefined
    const handle =
      node.tag === "secret-value"
        ? liftGuestSecretHandleFromWire(SECRET_INTERNAL, raw as any, node)
        : node.tag === "quota-token-handle"
          ? liftGuestQuotaTokenHandleFromWire(QUOTA_INTERNAL, raw as any, node)
          : liftGuestPermissionCardHandleFromWire(PERMISSION_CARD_INTERNAL, raw as any, node)
    this.transaction.lock(handle)
    this.transaction.register(() => {
      node.val = takeToWire(node.tag, handle, node)
    })
    return handle
  }

  stream(node: { val: SchemaValueStream | undefined }, itemCodec: ConcreteCodec): unknown {
    const raw = node.val
    if (raw === null || typeof raw !== "object" || this.raw.has(raw))
      throw new TypeError("invalid or aliased stream")
    this.raw.add(raw)
    const handle = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
      kind: "wrapped",
      value: raw,
    })
    node.val = undefined
    const ownership = handle.ownership(STREAM_INTERNAL)
    this.transaction.lock(ownership)
    this.transaction.register(() => {
      node.val = handle.take()?.value
    })
    return directAgentStreamFromHandle(handle, streamItemCodec(itemCodec), this.context)
  }

  finish(): void {
    if (this.visited.size !== this.tree.valueNodes.length)
      throw new TypeError("unreachable wire nodes")
  }
}

export class WireWriter {
  readonly valueNodes: SchemaValueNode[] = []
  constructor(
    private readonly context: Context.Context<any> = Context.empty() as Context.Context<any>,
  ) {}

  private readonly resources: Array<{
    node: any
    tag: OwnedTag
    handle: OwnedHandle
    transaction: CapabilityTransaction
  }> = []
  private readonly streams: Array<{
    node: any
    handle: GuestSchemaValueStreamHandle
    owner: object
  }> = []

  add(node: SchemaValueNode): number {
    return this.valueNodes.push(node) - 1
  }

  resource(tag: OwnedTag, value: OwnedHandle): number {
    const raw = peek(tag, value)
    if (raw === undefined) throw new TypeError(`${tag} was already transferred`)
    if (this.resources.some((entry) => peek(entry.tag, entry.handle) === raw))
      throw new TypeError("aliased resource")
    const transaction = new CapabilityTransaction()
    transaction.lock(value)
    const node = { tag, val: undefined } as any
    this.resources.push({ node, tag, handle: value, transaction })
    return this.add(node)
  }

  stream(value: any, itemCodec: ConcreteCodec): number {
    const handle = directAgentStreamToHandle(value, streamItemCodec(itemCodec), this.context)
    const owner = {}
    handle.reserve(owner)
    const node = { tag: "stream-value", val: undefined } as any
    this.streams.push({ node, handle, owner })
    return this.add(node)
  }

  trial(write: () => number): number | undefined {
    const nodes = this.valueNodes.length
    const resources = this.resources.length
    const streams = this.streams.length
    try {
      return write()
    } catch {
      this.valueNodes.length = nodes
      for (const entry of this.resources.splice(resources).reverse()) entry.transaction.rollback()
      for (const entry of this.streams.splice(streams).reverse())
        entry.handle.unreserve(entry.owner)
      return undefined
    }
  }

  finish(): void {
    if (this.streams.some(({ handle }) => handle.peek()?.kind !== "wrapped"))
      throw new TypeError("native schema streams require asynchronous encoding")
    this.commitResources()
    for (const { node, handle, owner } of this.streams) node.val = handle.take(owner)?.value
    for (const entry of this.resources) entry.transaction.commit()
  }

  async finishAsync(signal?: AbortSignal): Promise<void> {
    const prepared: Array<{ proxy: PreparedStream<SchemaValueTree>; wrapped?: SchemaValueStream }> =
      []
    let aborted = false
    const dispose = (value: SchemaValueStream) => {
      try {
        ;(value as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.()
      } catch {
        /* preserve failure */
      }
    }
    const abort = () => {
      if (aborted) return
      aborted = true
      for (const entry of prepared) {
        entry.proxy.abort()
        if (entry.wrapped) dispose(entry.wrapped)
      }
    }
    const check = () => {
      if (aborted || signal?.aborted) throw new Error("stream conversion interrupted")
    }
    signal?.addEventListener("abort", abort, { once: true })
    try {
      for (const entry of this.streams) {
        check()
        const endpoint = entry.handle.peek()
        if (endpoint?.kind === "native") {
          const pending = {
            proxy: new PreparedStream(endpoint.value),
            wrapped: undefined as SchemaValueStream | undefined,
          }
          prepared.push(pending)
          const { SchemaValueStream } = await import("golem:core/types@2.0.0")
          check()
          const value = await SchemaValueStream.wrap(pending.proxy)
          if (aborted) dispose(value)
          check()
          pending.wrapped = value
          entry.node.val = value
        } else entry.node.val = endpoint?.value
      }
      check()
      if (prepared.some(({ proxy }) => !proxy.pending))
        throw new TypeError("stream preparation was closed before handoff")
      this.commitResources()
      for (const entry of this.streams) entry.handle.take(entry.owner)
      for (const entry of prepared) entry.proxy.commit()
    } catch (error) {
      abort()
      throw error
    } finally {
      signal?.removeEventListener("abort", abort)
    }
  }

  rollback(): void {
    for (const entry of this.resources.slice().reverse()) entry.transaction.rollback()
    for (const entry of this.streams.slice().reverse()) entry.handle.unreserve(entry.owner)
  }

  private commitResources(): void {
    for (const entry of this.resources) entry.transaction.commit()
    for (const { node, tag, handle } of this.resources) node.val = takeToWire(tag, handle, node)
  }
}

const peek = (tag: OwnedTag, handle: OwnedHandle): object | undefined =>
  tag === "secret-value"
    ? peekGuestSecretHandle(SECRET_INTERNAL, handle as GuestSecretHandle)
    : tag === "quota-token-handle"
      ? peekGuestQuotaTokenHandle(QUOTA_INTERNAL, handle as GuestQuotaTokenHandle)
      : peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handle as GuestPermissionCardHandle)

const takeToWire = (tag: OwnedTag, handle: OwnedHandle, owner: object): object | undefined =>
  tag === "secret-value"
    ? takeGuestSecretHandleToWire(SECRET_INTERNAL, handle as GuestSecretHandle, owner)
    : tag === "quota-token-handle"
      ? takeGuestQuotaTokenHandleToWire(QUOTA_INTERNAL, handle as GuestQuotaTokenHandle, owner)
      : takeGuestPermissionCardHandleToWire(
          PERMISSION_CARD_INTERNAL,
          handle as GuestPermissionCardHandle,
          owner,
        )

const schemaError = (error: unknown) =>
  new Schema.SchemaError(new SchemaIssue.InvalidValue({ message: String(error) }))

export function writeConcrete(
  codec: ConcreteCodec,
  value: unknown,
  context: Context.Context<any> = Context.empty() as Context.Context<any>,
): SchemaValueTree {
  const writer = new WireWriter(context)
  try {
    const root = codec.write(value, writer)
    writer.finish()
    return { valueNodes: writer.valueNodes, root }
  } catch (error) {
    writer.rollback()
    throw error
  }
}

export const writeConcreteAsync = (codec: ConcreteCodec, value: unknown) =>
  Effect.flatMap(Effect.context<any>(), (context) =>
    Effect.tryPromise({
      try: async (signal) => {
        const writer = new WireWriter(context)
        try {
          const root = codec.write(value, writer)
          await writer.finishAsync(signal)
          return { valueNodes: writer.valueNodes, root }
        } catch (error) {
          writer.rollback()
          throw error
        }
      },
      catch: schemaError,
    }),
  )

export const readConcrete = (codec: ConcreteCodec, tree: SchemaValueTree) =>
  Effect.flatMap(Effect.context<any>(), (context) =>
    Effect.tryPromise({
      try: async () => {
        const transaction = new CapabilityTransaction()
        try {
          const reader = new WireReader(tree, transaction, context)
          const result = codec.read(reader, tree.root)
          reader.finish()
          transaction.commit()
          return result
        } catch (error) {
          transaction.rollback()
          await discard(tree)
          throw error
        }
      },
      catch: schemaError,
    }),
  )

async function discard(tree: SchemaValueTree): Promise<void> {
  if (!Array.isArray(tree?.valueNodes)) return
  const seen = new Set<object>()
  for (const node of tree.valueNodes as any[]) {
    if (
      !["secret-value", "quota-token-handle", "permission-card-handle", "stream-value"].includes(
        node?.tag,
      )
    )
      continue
    const raw = node.val
    node.val = undefined
    if (!raw || typeof raw !== "object" || seen.has(raw)) continue
    seen.add(raw)
    if (node.tag === "quota-token-handle")
      abandonGuestQuotaTokenWireHandle(QUOTA_INTERNAL, raw, node)
    else if (node.tag === "permission-card-handle")
      abandonGuestPermissionCardWireHandle(PERMISSION_CARD_INTERNAL, raw, node)
    try {
      if (node.tag === "stream-value") {
        const source = await (await import("golem:core/types@2.0.0")).SchemaValueStream.unwrap(raw)
        await source[Symbol.asyncIterator]().return?.()
      }
    } catch {
      /* drain siblings */
    } finally {
      try {
        raw[Symbol.dispose]?.()
      } catch {
        /* drain siblings */
      }
    }
  }
}

export function compiledWire<S extends Schema.Top>(
  schema: S,
  graph: SchemaGraph | undefined,
  codec: ConcreteCodec,
) {
  return {
    schemaGraph: graph,
    encode: (value: S["Type"]) =>
      Schema.encodeEffect(schema)(value).pipe(
        Effect.flatMap((encoded) =>
          Effect.flatMap(Effect.context<any>(), (context) =>
            Effect.try({ try: () => writeConcrete(codec, encoded, context), catch: schemaError }),
          ),
        ),
      ),
    encodeAsync: (value: S["Type"]) =>
      Schema.encodeEffect(schema)(value).pipe(
        Effect.flatMap((encoded) => writeConcreteAsync(codec, encoded)),
      ),
    decode: (tree: SchemaValueTree, check?: () => void) =>
      Effect.flatMap(Effect.context<any>(), (context) => {
        const transaction = new CapabilityTransaction()
        const conversion = Effect.flatMap(
          Effect.try({
            try: () => {
              check?.()
              const reader = new WireReader(tree, transaction, context)
              const encoded = codec.read(reader, tree.root)
              reader.finish()
              return encoded
            },
            catch: schemaError,
          }),
          (encoded) => Schema.decodeEffect(schema)(encoded as S["Encoded"]),
        )
        return Effect.onExit(conversion, (exit) =>
          Effect.suspend(() => {
            if (Exit.isSuccess(exit)) {
              transaction.commit()
              return Effect.void
            }
            transaction.rollback()
            return Effect.promise(() => discard(tree))
          }),
        )
      }),
  }
}

function streamItemCodec(codec: ConcreteCodec) {
  if (codec.schema) {
    const wire = compiledWire(codec.schema, undefined, codec)
    return { encode: wire.encodeAsync, decode: wire.decode }
  }
  return {
    encode: (value: unknown) => writeConcreteAsync(codec, value),
    decode: (tree: SchemaValueTree) => readConcrete(codec, tree),
  }
}
