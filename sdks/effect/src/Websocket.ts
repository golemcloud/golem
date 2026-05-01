/**
 * Effect-idiomatic façade over `golem:websocket/client@1.5.0`.
 *
 * The Golem host exposes a durable WebSocket client (a resource type
 * with `connect / send / receive / receive-with-timeout / close /
 * subscribe`). This module bridges that resource to the canonical
 * Effect v4 {@link https://github.com/Effect-TS/effect | Socket}
 * abstraction (`effect/unstable/socket`), so user code can plug
 * Golem-backed websockets into the same `Stream` / `Channel`
 * combinators used against the browser- / Node-backed adapters.
 *
 * Wire-compatible with the official `golem-rust.WebsocketConnection`
 * helper: every `subscribe()` → `receive()` round-trip is a single
 * oplog entry, and `closed` is treated as terminal (subsequent
 * `receive()` calls fail with the stored close info).
 *
 * **Lifecycle model.** {@link connect} is **eager** and **scoped** —
 * the underlying host `WebsocketConnection` is acquired immediately
 * and released by the surrounding `Scope`. The returned
 * {@link Socket.Socket}'s `writer` is suspended on an internal latch
 * until a `run*` call (or a {@link makeChannel} wiring) is active;
 * issuing writes before the read loop has been started will **block
 * indefinitely** with the current implementation. The recommended
 * pattern is therefore to fork the read loop first, then acquire the
 * writer.
 *
 * **`openTimeout`** is currently a documented no-op: Golem's host
 * `connect(...)` is synchronous (it returns only after the handshake
 * is complete or has failed), so there is no separate "open" event
 * to await.
 *
 * **Example** (fork the read loop, then write)
 *
 * ```ts
 * import { Effect, Fiber } from "effect"
 * import { Socket } from "effect/unstable/socket"
 * import { Websocket } from "effect-golem"
 *
 * const drain = Effect.scoped(
 *   Effect.gen(function* () {
 *     const sock = yield* Websocket.connect("wss://echo.example/ws", {
 *       closeCodeIsError: (c) => c !== 1000,
 *     })
 *     // Fork the read loop first — `writer` is gated on it being
 *     // active, so writing before this fork would suspend forever.
 *     const reader = yield* Effect.forkChild(
 *       sock.runString((line) =>
 *         Effect.logInfo("recv").pipe(Effect.annotateLogs({ line })),
 *       ),
 *     )
 *     const write = yield* sock.writer
 *     yield* write("hello")
 *     yield* write(new Socket.CloseEvent(1000, "bye"))
 *     yield* Fiber.join(reader)
 *   }),
 * )
 * ```
 *
 * **Example** (as a `Channel`)
 *
 * ```ts
 * const channel = Websocket.makeChannel("wss://echo.example/ws")
 * ```
 *
 * @since 1.5.0
 */

import type { NonEmptyReadonlyArray } from "effect/Array"
import * as Channel from "effect/Channel"
import * as Context from "effect/Context"
import * as Deferred from "effect/Deferred"
import type * as Duration from "effect/Duration"
import * as Effect from "effect/Effect"
import * as FiberSet from "effect/FiberSet"
import * as Latch from "effect/Latch"
import * as Layer from "effect/Layer"
import * as Scope from "effect/Scope"
import { Socket } from "effect/unstable/socket"
import type * as WsClient from "golem:websocket/client@1.5.0"
import { WebsocketClient } from "./host/WebsocketClient.js"

// ---------------------------------------------------------------------------
// Error mapping (golem:websocket error → Socket.SocketError)
// ---------------------------------------------------------------------------

/**
 * Local shadow of `golem:websocket/client@1.5.0.error`. Tag union is
 * derived from the WIT type, so a new variant in the regenerated d.ts
 * shows up here as a missing case in {@link mapToSocketError}'s switch
 * (caught by `noImplicitReturns`).
 */
interface TaggedWsError {
  readonly tag: WsClient.Error["tag"]
  readonly val?: unknown
}

const WS_TAGS = new Set<WsClient.Error["tag"]>([
  "connection-failure",
  "send-failure",
  "receive-failure",
  "protocol-error",
  "closed",
  "other",
])

const isTaggedWsError = (e: unknown): e is TaggedWsError => {
  if (e === null || typeof e !== "object") return false
  const obj = e as { tag?: unknown }
  return typeof obj.tag === "string" && WS_TAGS.has(obj.tag as TaggedWsError["tag"])
}

const extractWsError = (e: unknown): TaggedWsError | undefined => {
  if (isTaggedWsError(e)) return e
  if (e instanceof Error) {
    const payload = (e as unknown as { payload?: unknown }).payload
    if (isTaggedWsError(payload)) return payload
    const cause = (e as unknown as { cause?: unknown }).cause
    if (isTaggedWsError(cause)) return cause
  }
  return undefined
}

/**
 * Build a JSON-friendly cause value for the `Schema.Defect` slot on
 * `SocketOpenError` / `SocketWriteError` / `SocketReadError`. We
 * deliberately avoid wrapping primitives in `Error(...)`, because the
 * built-in `Error` type's `.message` is not enumerable and so
 * `JSON.stringify(cause)` would lose the originating string in
 * tests / logs.
 */
const ensureCause = (e: unknown): unknown => {
  if (e === undefined || e === null) return "unknown error"
  if (typeof e === "string" || typeof e === "number" || typeof e === "boolean") return e
  if (e instanceof Error) {
    // Serialize the relevant Error fields so JSON.stringify doesn't
    // collapse to "{}".
    return { name: e.name, message: e.message }
  }
  return e
}

const errorForCallSite = (
  fallback: "open" | "send" | "receive",
  raw: unknown,
): Socket.SocketError => {
  switch (fallback) {
    case "open":
      return new Socket.SocketError({
        reason: new Socket.SocketOpenError({ kind: "Unknown", cause: ensureCause(raw) }),
      })
    case "send":
      return new Socket.SocketError({
        reason: new Socket.SocketWriteError({ cause: ensureCause(raw) }),
      })
    case "receive":
      return new Socket.SocketError({
        reason: new Socket.SocketReadError({ cause: ensureCause(raw) }),
      })
  }
}

const mapToSocketError = (
  cause: unknown,
  fallback: "open" | "send" | "receive",
): Socket.SocketError => {
  const tagged = extractWsError(cause)
  if (tagged !== undefined) {
    switch (tagged.tag) {
      // Inherently call-site-bound tags emitted by exactly one host
      // entrypoint — keep their classification regardless of where
      // they bubble up from.
      case "connection-failure":
        return new Socket.SocketError({
          reason: new Socket.SocketOpenError({
            kind: "Unknown",
            cause: ensureCause(tagged.val),
          }),
        })
      case "send-failure":
        return new Socket.SocketError({
          reason: new Socket.SocketWriteError({
            cause: ensureCause(tagged.val),
          }),
        })
      case "receive-failure":
        return new Socket.SocketError({
          reason: new Socket.SocketReadError({
            cause: ensureCause(tagged.val),
          }),
        })
      // Ambiguous tags: the host can emit `protocol-error` or
      // `other` from connect / send / receive alike. Defer the
      // classification to the failing call site so a transport-level
      // protocol violation observed during `connect(...)` does NOT
      // get mis-reported as a `SocketReadError`, etc.
      case "protocol-error":
      case "other":
        return errorForCallSite(fallback, tagged.val)
      case "closed": {
        const info = tagged.val as { code?: number; reason?: string } | undefined
        return new Socket.SocketError({
          reason: new Socket.SocketCloseError({
            code: info?.code ?? 1000,
            closeReason: info?.reason,
          }),
        })
      }
    }
  }
  // Unknown shape — classify by failing call site.
  return errorForCallSite(fallback, cause)
}

// ---------------------------------------------------------------------------
// Public options
// ---------------------------------------------------------------------------

/**
 * Options for {@link connect}, {@link layer}, {@link makeChannel}.
 *
 * - `headers` is forwarded verbatim to the host's
 *   `WebsocketConnection.connect(url, headers)` call. Use this slot to
 *   supply auth tokens, `Sec-WebSocket-Protocol` (subprotocols), etc.
 * - `closeCodeIsError` mirrors Effect's stock socket adapters: any
 *   close code for which this returns `false` is treated as a clean
 *   shutdown, the read loop terminates with `Effect.void`, and the
 *   `Socket.SocketCloseError` is filtered out of the failure channel.
 *   Default: every close code is an error (matches `Socket.defaultCloseCodeIsError`).
 * - `openTimeout` is accepted for API parity with
 *   `Socket.makeWebSocket` but is not currently consulted: the Golem
 *   host's `connect` is synchronous (it does not return until the
 *   handshake is complete or has failed), so there is no separate
 *   "open" event to await.
 *
 * @since 1.5.0
 * @category models
 */
export interface ConnectOptions {
  readonly headers?: ReadonlyArray<readonly [string, string]> | undefined
  readonly closeCodeIsError?: ((code: number) => boolean) | undefined
  readonly openTimeout?: Duration.Input | undefined
}

const toHostHeaders = (
  headers: ReadonlyArray<readonly [string, string]> | undefined,
): [string, string][] | undefined =>
  headers === undefined ? undefined : headers.map(([k, v]) => [k, v] as [string, string])

// ---------------------------------------------------------------------------
// fromConnection — turn an effect that produces a WebsocketConnection
// into a Socket (the lower-level building block).
// ---------------------------------------------------------------------------

/**
 * Build a {@link Socket.Socket} from an Effect that acquires a Golem
 * `WebsocketConnection`. Lower-level than {@link connect}; useful when
 * you want full control over the resource lifecycle (e.g. reusing a
 * persisted connection across invocations).
 *
 * @since 1.5.0
 * @category constructors
 */
export const fromConnection = <RO>(
  acquire: Effect.Effect<WsClient.WebsocketConnection, Socket.SocketError, RO>,
  options?: { readonly closeCodeIsError?: ((code: number) => boolean) | undefined } | undefined,
): Effect.Effect<Socket.Socket, never, Exclude<RO, Scope.Scope>> =>
  Effect.withFiber((fiber) => {
    let currentWS: WsClient.WebsocketConnection | undefined
    let currentDeferred: Deferred.Deferred<unknown, unknown> | undefined
    const latch = Latch.makeUnsafe(false)
    const acquireServices = fiber.context as Context.Context<RO>
    const closeCodeIsError = options?.closeCodeIsError ?? Socket.defaultCloseCodeIsError

    const runRaw = <_, E, R>(
      handler: (_: string | Uint8Array) => Effect.Effect<_, E, R> | void,
      opts?: { readonly onOpen?: Effect.Effect<void> | undefined },
    ) =>
      Effect.scopedWith(
        Effect.fnUntraced(function* (scope) {
          const ws = yield* Scope.provide(acquire, scope)
          const fiberSet = yield* FiberSet.make<unknown, E | Socket.SocketError>().pipe(
            Scope.provide(scope),
          )
          const runFork = yield* FiberSet.runtime(fiberSet)<R>()

          yield* Effect.tryPromise({
            // The signal here fires when the surrounding fiber is
            // interrupted (Effect's `tryPromise` wires its own
            // AbortController to the fiber's interrupt observer).
            // Threading it through `pollable.abortablePromise(signal)`
            // makes the read loop wake up promptly on interruption
            // instead of leaking a host promise that will never
            // resolve.
            try: async (signal) => {
              while (true) {
                if (signal.aborted) return
                // Wait for a message to be available. The pollable's
                // abortablePromise() resolves when the host has
                // buffered a frame, or rejects if `signal` aborts.
                const pollable = ws.subscribe()
                try {
                  await pollable.abortablePromise(signal)
                } catch (err) {
                  if (signal.aborted) return
                  throw err
                }
                if (signal.aborted) return
                // Now `receive()` is guaranteed non-blocking. Any thrown
                // error here (including `closed`) terminates the loop;
                // it will be classified by the catch handler below.
                const msg = ws.receive()
                const data: string | Uint8Array = msg.tag === "text" ? msg.val : msg.val
                const result = handler(data)
                if (Effect.isEffect(result)) runFork(result)
              }
            },
            catch: (cause) => mapToSocketError(cause, "receive"),
          }).pipe(FiberSet.run(fiberSet))

          currentWS = ws
          currentDeferred = fiberSet.deferred as Deferred.Deferred<unknown, unknown>
          yield* latch.open
          if (opts?.onOpen) yield* opts.onOpen

          return yield* Effect.catchFilter(
            FiberSet.join(fiberSet),
            Socket.SocketCloseError.filterClean((c) => !closeCodeIsError(c)),
            () => Effect.void,
          )
        }),
      ).pipe(
        Effect.updateContext((input: Context.Context<R>) => Context.merge(acquireServices, input)),
        Effect.ensuring(
          Effect.sync(() => {
            latch.closeUnsafe()
            currentWS = undefined
            currentDeferred = undefined
          }),
        ),
      )

    const write = (chunk: Uint8Array | string | Socket.CloseEvent) =>
      latch.whenOpen(
        Effect.suspend(() => {
          const ws = currentWS!
          if (Socket.isCloseEvent(chunk)) {
            // Best-effort close on the host side. Don't rely on the
            // host to surface a synthetic `error::closed` to wake
            // the read loop — instead, fail the read fiber's
            // deferred ourselves so `runRaw` returns promptly even
            // if the host is unresponsive or deliberately silent on
            // local close.
            try {
              ws.close(chunk.code, chunk.reason)
            } catch {
              // swallow — the read loop is the source of truth
            }
            const closeError = new Socket.SocketError({
              reason: new Socket.SocketCloseError({
                code: chunk.code,
                closeReason: chunk.reason,
              }),
            })
            const deferred = currentDeferred
            return deferred === undefined
              ? Effect.void
              : Deferred.fail(deferred, closeError).pipe(Effect.asVoid)
          }
          return Effect.try({
            try: () => {
              if (typeof chunk === "string") {
                ws.send({ tag: "text", val: chunk })
              } else {
                ws.send({ tag: "binary", val: chunk })
              }
            },
            catch: (cause) => mapToSocketError(cause, "send"),
          })
        }),
      )

    const writer = Effect.succeed(write)

    return Effect.succeed(
      Socket.make({
        runRaw,
        writer,
      }),
    )
  })

// ---------------------------------------------------------------------------
// connect — the high-level constructor most users want.
// ---------------------------------------------------------------------------

/**
 * Connect to a WebSocket server at `url` (`ws://` or `wss://`).
 *
 * The returned Effect is scoped: the underlying host
 * `WebsocketConnection` is acquired immediately and a finalizer that
 * issues `close(1000, undefined)` is registered on the surrounding
 * `Scope`. Wrap with `Effect.scoped` to acquire and release in one
 * go, or compose into a longer-lived scope to share the connection
 * across multiple `Socket.run*` invocations.
 *
 * Failure cases:
 *
 * - `connection-failure` from the host → `SocketError(SocketOpenError)`
 * - any other thrown error during `connect(...)` → `SocketError(SocketOpenError)`
 *
 * Wire-compatible with `golem-rust.WebsocketConnection::connect(...)`.
 *
 * @since 1.5.0
 * @category constructors
 */
export const connect = (
  url: string,
  options?: ConnectOptions,
): Effect.Effect<Socket.Socket, Socket.SocketError, Scope.Scope | WebsocketClient> =>
  Effect.gen(function* () {
    const client = yield* WebsocketClient
    // Open the WebSocket eagerly. Golem's `connect(...)` is
    // synchronous — it returns only after the handshake is complete
    // (or has failed) — so there is no separate "open" event to
    // await. Failure surfaces here as `SocketError(SocketOpenError)`.
    const ws = yield* Effect.acquireRelease(
      Effect.try({
        try: () => client.connect(url, toHostHeaders(options?.headers)),
        catch: (cause) => mapToSocketError(cause, "open"),
      }),
      (ws) =>
        Effect.sync(() => {
          try {
            ws.close(1000, undefined)
          } catch {
            // swallow — the connection may already be closed
          }
        }),
    )
    return yield* fromConnection(Effect.succeed(ws), options)
  })

// ---------------------------------------------------------------------------
// layer — Layer<Socket, SocketError>
// ---------------------------------------------------------------------------

/**
 * `Layer<Socket.Socket, SocketError>` that connects to `url` on
 * activation and provides the resulting socket to the layer scope.
 * Equivalent to `Socket.layerWebSocket(...)` in the canonical
 * browser/Node setup.
 *
 * @since 1.5.0
 * @category layers
 */
export const layer = (
  url: string,
  options?: ConnectOptions,
): Layer.Layer<Socket.Socket, Socket.SocketError, WebsocketClient> =>
  Layer.effect(Socket.Socket)(connect(url, options))

// ---------------------------------------------------------------------------
// makeChannel — build a duplex Channel directly.
// ---------------------------------------------------------------------------

/**
 * Build a duplex `Channel` whose:
 *
 * - **outputs** are non-empty arrays of `Uint8Array` decoded from the
 *   inbound websocket frames;
 * - **inputs** are non-empty arrays of `Uint8Array | string |
 *   Socket.CloseEvent` to send outbound (close events trigger
 *   `WebsocketConnection.close(...)`);
 * - **errors** are `SocketError | IE`.
 *
 * Equivalent to `Socket.makeWebSocketChannel(url, options)` in the
 * canonical browser/Node setup. Combine with
 * `Stream.fromChannel(...)` / `Stream.pipeThroughChannel(...)` for
 * stream-based pipelines.
 *
 * @since 1.5.0
 * @category constructors
 */
export const makeChannel = <IE = never>(
  url: string,
  options?: ConnectOptions,
): Channel.Channel<
  NonEmptyReadonlyArray<Uint8Array>,
  Socket.SocketError | IE,
  void,
  NonEmptyReadonlyArray<Uint8Array | string | Socket.CloseEvent>,
  IE,
  unknown,
  WebsocketClient
> => Channel.unwrap(Effect.scoped(Effect.map(connect(url, options), Socket.toChannelWith<IE>())))
