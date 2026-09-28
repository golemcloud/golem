/** Effect-native external Durable Streams. @since 1.6.0 */
import { Effect, Option, Schema, Scope, Semaphore, Stream } from "effect"
import type * as Host from "golem:agent/durable-streams@2.0.0"
import type { Secret } from "golem:core/types@2.0.0"
import { DurableStreamsClient } from "./host/DurableStreamsClient.js"
import { SchemaRef } from "./SchemaRef.js"
import { compile } from "./WitCodec.js"

/** Sanitized protocol failure, never an end-of-stream marker. @since 1.6.0 @category errors */
export class DurableStreamError extends Error {
  readonly _tag = "DurableStreamError"
  constructor(
    readonly kind: Host.DurableStreamErrorKind,
    message: string,
    readonly retryAfterMs?: bigint,
    readonly producerEpoch?: bigint,
    readonly expectedSequence?: bigint,
  ) {
    super(message)
  }
}

/** Acknowledgement; duplicates may omit nextOffset. @since 1.6.0 @category models */
export type AppendReceipt = Host.DurableStreamAppendReceipt

/** Shared connection and per-operation retry policy. @since 1.6.0 @category models */
export interface Options {
  readonly url: string
  /** Borrowed string-secret capability, never revealed by this SDK. */
  readonly auth?: Secret
  /** Whole attempt deadline, 1–300000 ms; default 30000. */
  readonly timeoutMs?: number
  /** Automatic retries, excluding the initial attempt; default 5. */
  readonly maxRetries?: number
  /** Initial exponential delay, capped at 30000 ms; default 100. */
  readonly retryDelayMs?: number
}

/** Read from an opaque checkpoint, then follow live batches. @since 1.6.0 @category models */
export interface ReadOptions extends Options {
  /** Defaults to -1. The special offset now is resolved once by catch-up. */
  readonly offset?: string
  readonly cursor?: string
  readonly live?: "long-poll" | "sse"
  /** Delay after an empty up-to-date batch; default 100 ms. */
  readonly idleDelayMs?: number
}

/** Producer identity is allocated once, not again after fork or retry. @since 1.6.0 @category models */
export interface WriteOptions extends Options {
  /** Defaults to the runtime's durable crypto.randomUUID(). */
  readonly producerId?: string
  /** Nonnegative protocol integer through 2^53-1; default 0. */
  readonly epoch?: bigint
  readonly contentType?: string
}

/**
 * Optional deterministic exact JSON conversion for a single message (not the outer batch).
 * Schema validation still applies. Use this for unquoted integers beyond the safe JS range.
 * @since 1.6.0 @category codecs
 */
export interface JsonCodec<A> {
  readonly decode?: (json: string) => A
  readonly encode?: (value: A) => string
}

/**
 * A serialized producer. Failure or interruption retains the exact request; only retryPending
 * may resolve it. Reconstruct through deterministic execution, not a snapshot of this object.
 * Keep its acquisition scope open through all appends and retries; scope exit drops the resource.
 * @since 1.6.0 @category models
 */
export interface Writer<A, E = never, R = never> {
  readonly producerId: string
  readonly epoch: bigint
  readonly nextSequence: Effect.Effect<bigint>
  readonly hasPending: Effect.Effect<boolean>
  readonly append: (
    data: A,
    options?: { readonly close?: boolean },
  ) => Effect.Effect<AppendReceipt, DurableStreamError | E, R>
  readonly close: Effect.Effect<AppendReceipt, DurableStreamError>
  readonly retryPending: Effect.Effect<AppendReceipt, DurableStreamError>
}

/** Lazy native Stream of individual bytes, not append boundaries. @since 1.6.0 @category readers */
export const readBytes = (options: ReadOptions) =>
  read(options, "bytes", (payload) => Effect.succeed(Array.from(payload)))

/** Schema-checked messages; array-valued messages remain one element. @since 1.6.0 @category readers */
export const readJson = <S extends Schema.Top>(
  schema: S,
  options: ReadOptions & Pick<JsonCodec<S["Type"]>, "decode">,
) =>
  Stream.unwrap(
    Effect.gen(function* () {
      const codec = yield* compile(schema)
      const ref = new SchemaRef(codec.schemaGraph)
      return read(options, "json", (payload) =>
        jsonTry(() => splitJsonBatch(payload), "protocol-error"),
      ).pipe(
        Stream.rechunk(1),
        Stream.mapEffect((json) =>
          Effect.gen(function* () {
            if (options.decode === undefined) {
              const tree = yield* jsonTry(() => ref.packJson(parseSafeJson(json)), "protocol-error")
              return yield* codec.decode(tree)
            }
            const value = yield* jsonTry(() => options.decode!(json), "protocol-error")
            const tree = yield* codec.encode(value)
            if (!ref.validateValue(tree).success)
              return yield* invalid(
                "protocol-error",
                "Decoded message does not conform to its schema",
              )
            return value
          }),
        ),
      )
    }),
  )

/** Acquire one scoped producer resource for exact bytes. @since 1.6.0 @category writers */
export const makeByteWriter = (options: WriteOptions) =>
  makeWriter(
    options,
    "application/octet-stream",
    { tag: "bytes", val: new Uint8Array() },
    (bytes: Uint8Array) => Effect.sync(() => ({ tag: "bytes" as const, val: bytes.slice() })),
  )

/** Acquire one scoped schema-checked JSON producer resource. @since 1.6.0 @category writers */
export const makeJsonWriter = <S extends Schema.Top>(
  schema: S,
  options: WriteOptions & Pick<JsonCodec<S["Type"]>, "encode">,
) =>
  Effect.gen(function* () {
    const codec = yield* compile(schema)
    const ref = new SchemaRef(codec.schemaGraph)
    return yield* makeWriter(
      options,
      "application/json",
      { tag: "json", val: [] },
      (values: ReadonlyArray<S["Type"]>) =>
        Effect.gen(function* () {
          const encoded = yield* Effect.forEach(values, (value) =>
            Effect.gen(function* () {
              const tree = yield* codec.encode(value)
              if (!ref.validateValue(tree).success)
                return yield* invalid("invalid-request", "Message does not conform to its schema")
              return yield* jsonTry(() => {
                const json =
                  options.encode === undefined
                    ? JSON.stringify(ref.unpackJson(tree))
                    : options.encode(value)
                if (options.encode === undefined) parseSafeJson(json)
                else JSON.parse(json)
                return json
              }, "invalid-request")
            }),
          )
          return { tag: "json" as const, val: encoded }
        }),
    )
  })

const read = <A, E, R>(
  input: ReadOptions,
  mode: Host.DurableStreamMode,
  decode: (payload: Uint8Array) => Effect.Effect<ReadonlyArray<A>, E, R>,
) =>
  Stream.unwrap(
    Effect.gen(function* () {
      const options = yield* checkedOptions(input)
      const idleDelay = yield* bounded(input.idleDelayMs ?? 100, 1, 300000, "idleDelayMs")
      const host = yield* DurableStreamsClient
      const reader = yield* host.makeReader(
        { url: options.url, mode, timeoutMs: BigInt(options.timeoutMs) },
        options.auth,
      )
      const initial: { request: Host.DurableStreamReadRequest; idle: boolean } = {
        request: {
          checkpoint: { offset: input.offset ?? "-1", cursor: input.cursor },
          transport: "catch-up",
          contentType: undefined,
        },
        idle: false,
      }
      return Stream.paginate(initial, (state) =>
        Effect.gen(function* () {
          if (state.idle) yield* Effect.sleep(idleDelay)
          const batch = yield* retry(options, reader.read(state.request))
          if (batch.next.offset === "now")
            return yield* invalid("protocol-error", "Server did not resolve now")
          const items = yield* decode(batch.payload)
          return [
            items,
            batch.closed
              ? Option.none()
              : Option.some({
                  request: {
                    ...state.request,
                    checkpoint: { ...batch.next },
                    contentType: state.request.contentType ?? batch.contentType,
                    transport: batch.upToDate ? (input.live ?? "long-poll") : "catch-up",
                  },
                  idle: items.length === 0 && batch.upToDate,
                }),
          ] as const
        }),
      )
    }),
  )

const makeWriter = <A, E, R>(
  input: WriteOptions,
  defaultContentType: string,
  empty: Host.DurableStreamAppendPayload,
  encode: (data: A) => Effect.Effect<Host.DurableStreamAppendPayload, E, R>,
): Effect.Effect<Writer<A, E, R>, DurableStreamError, DurableStreamsClient | Scope.Scope> =>
  Effect.gen(function* () {
    const options = yield* checkedOptions(input)
    const producerId = input.producerId ?? crypto.randomUUID()
    const epoch = input.epoch ?? 0n
    const contentType = input.contentType ?? defaultContentType
    if (!producerId || epoch < 0n || epoch > MAX_INTEGER)
      return yield* invalid("invalid-request", "Invalid producer ID or epoch")
    const host = yield* DurableStreamsClient
    const writer = yield* host.makeWriter(
      {
        url: options.url,
        contentType,
        producerId,
        producerEpoch: epoch,
        timeoutMs: BigInt(options.timeoutMs),
      },
      options.auth,
    )
    const lock = yield* Semaphore.make(1)
    let nextSequence = 0n
    let closed = false
    let pending: { request: Host.DurableStreamAppendRequest; retry: RetryState } | undefined
    const resolve = Effect.gen(function* () {
      const current = pending
      if (current === undefined) return yield* invalid("invalid-request", "No pending append")
      const { request } = current
      const receipt = yield* retry(options, writer.append(request), current.retry)
      if (receipt.epoch !== epoch || receipt.sequence < request.sequence)
        return yield* invalid("protocol-error", "Invalid producer acknowledgement")
      if (receipt.sequence > request.sequence)
        return yield* invalid("producer-diverged", "Another writer advanced this producer")
      if (request.close && !receipt.closed)
        return yield* invalid("protocol-error", "Peer did not acknowledge closure")
      nextSequence = request.sequence + 1n
      closed = receipt.closed
      pending = undefined
      return receipt
    })
    const append = <E2, R2>(
      payload: Effect.Effect<Host.DurableStreamAppendPayload, E2, R2>,
      close: boolean,
    ) =>
      lock.withPermits(1)(
        Effect.gen(function* () {
          if (pending !== undefined)
            return yield* invalid(
              "sequence-conflict",
              "Resolve the pending append before supplying new data",
            )
          if (closed) return yield* invalid("closed", "Writer is closed")
          if (nextSequence > MAX_INTEGER)
            return yield* invalid("sequence-conflict", "Producer sequence exhausted")
          const body = yield* payload
          if (body.val.length === 0 && !close)
            return yield* invalid("invalid-request", "Empty append requires close")
          pending = {
            request: {
              payload: body,
              sequence: nextSequence,
              close,
            },
            retry: { failures: 0 },
          }
          return yield* resolve
        }),
      )
    return {
      producerId,
      epoch,
      nextSequence: Effect.sync(() => nextSequence),
      hasPending: Effect.sync(() => pending !== undefined),
      append: (data, options) => append(encode(data), options?.close ?? false),
      close: append(Effect.succeed(empty), true),
      retryPending: lock.withPermits(1)(resolve),
    }
  })

const MAX_INTEGER = 9007199254740991n
const invalid = (kind: Host.DurableStreamErrorKind, message: string) =>
  Effect.fail(new DurableStreamError(kind, message))
const bounded = (value: number, min: number, max: number, name: string) =>
  Number.isSafeInteger(value) && value >= min && value <= max
    ? Effect.succeed(value)
    : invalid("invalid-request", `${name} must be an integer from ${min} through ${max}`)
const checkedOptions = (options: Options) =>
  Effect.gen(function* () {
    return {
      url: options.url,
      auth: options.auth,
      timeoutMs: yield* bounded(options.timeoutMs ?? 30000, 1, 300000, "timeoutMs"),
      maxRetries: yield* bounded(options.maxRetries ?? 5, 0, 1000, "maxRetries"),
      retryDelayMs: yield* bounded(options.retryDelayMs ?? 100, 1, 30000, "retryDelayMs"),
    }
  })
interface RetryState {
  failures: number
  delay?: bigint
}
const retry = <A>(
  options: { maxRetries: number; retryDelayMs: number },
  action: Effect.Effect<A, Host.DurableStreamError>,
  state: RetryState = { failures: 0 },
) => {
  const loop: Effect.Effect<A, DurableStreamError> = Effect.gen(function* () {
    if (state.delay !== undefined) {
      yield* sleep(state.delay)
      state.delay = undefined
    }
    return yield* action.pipe(
      Effect.catch((error) => {
        if (state.failures >= options.maxRetries || !retryable[error.kind])
          return Effect.fail(
            new DurableStreamError(
              error.kind,
              error.message,
              error.retryAfterMs,
              error.producerEpoch,
              error.expectedSequence,
            ),
          )
        const backoff = BigInt(Math.min(30000, options.retryDelayMs * 2 ** state.failures++))
        state.delay =
          error.retryAfterMs !== undefined && error.retryAfterMs > backoff
            ? error.retryAfterMs
            : backoff
        return loop
      }),
    )
  })
  return loop
}
const retryable = {
  "invalid-request": false,
  "permission-denied": false,
  "not-found": false,
  gone: false,
  closed: false,
  "sequence-conflict": false,
  fenced: false,
  "producer-diverged": false,
  "protocol-error": false,
  "payload-too-large": false,
  timeout: true,
  transport: true,
  "rate-limited": true,
  unavailable: true,
} satisfies Record<Host.DurableStreamErrorKind, boolean>
const sleep = (milliseconds: bigint): Effect.Effect<void> =>
  Effect.gen(function* () {
    let remaining = milliseconds
    while (remaining > 0n) {
      const chunk = remaining > 2147483647n ? 2147483647n : remaining
      yield* Effect.sleep(Number(chunk))
      remaining -= chunk
    }
  })
const jsonTry = <A>(run: () => A, kind: "invalid-request" | "protocol-error") =>
  Effect.try({
    try: run,
    catch: () => new DurableStreamError(kind, "Invalid JSON message or schema value"),
  })

// Preserve original numeric lexemes for exact application codecs.
const splitJsonBatch = (payload: Uint8Array): string[] => {
  if (payload.length === 0) return []
  const text = new TextDecoder("utf-8", { fatal: true }).decode(payload).trim()
  if (!Array.isArray(JSON.parse(text))) throw new Error("Expected JSON batch array")
  if (text.slice(1, -1).trim() === "") return []
  const values: string[] = []
  let start = 1
  let depth = 0
  let quoted = false
  for (let i = 1; i < text.length - 1; i++) {
    const c = text[i]
    if (quoted) {
      if (c === "\\") i++
      else if (c === '"') quoted = false
    } else if (c === '"') quoted = true
    else if (c === "[" || c === "{") depth++
    else if (c === "]" || c === "}") depth--
    else if (c === "," && depth === 0) {
      values.push(text.slice(start, i))
      start = i + 1
    }
  }
  values.push(text.slice(start, -1))
  return values
}
const parseSafeJson = (json: string) => {
  for (const match of json.matchAll(/"(?:[^"\\]|\\.)*"|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g)) {
    if (
      match[1] !== undefined &&
      Number.isInteger(Number(match[1])) &&
      !Number.isSafeInteger(Number(match[1]))
    )
      throw new Error("Unsafe JSON integer requires an exact codec")
  }
  return JSON.parse(json)
}
