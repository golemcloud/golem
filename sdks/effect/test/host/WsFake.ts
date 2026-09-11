/**
 * Layer-based test fake for {@link WebsocketClient}. Per-test
 * instance state (responder, one-shot connect-failure injector, and
 * the call recorder) is held in closure variables behind the layer
 * and surfaced as Effect-typed accessors / setters so tests stay
 * Effect-idiomatic.
 *
 * The underlying `WebsocketClient.connect` shape is synchronous (it
 * mirrors the WIT host call which throws on handshake failure), so
 * the closure cannot use `Ref.set` directly inside the synchronous
 * handler — Effect's `Ref` does not expose a synchronous `setUnsafe`.
 * The public surface is still Ref-shaped (`setResponder`,
 * `setNextConnectError`, `recordedConnects` all return `Effect`s) to
 * match the rest of the layer-mocks fakes.
 *
 * Use with `Effect.provide(eff, fake.layer)` once per test (NOT via
 * `it.layer(fake.layer)`, which would share state across the entire
 * `describe` block and cause cross-test interference).
 */

import { Effect, Layer, Option } from "effect"
import type * as WsClient from "golem:websocket/client@1.5.0"
import { WebsocketClient } from "../../src/host/WebsocketClient.js"
import * as WsMock from "../mocks/golem-websocket-client.js"

/**
 * Connect responder: maps `(url, headers)` to a `WebsocketConnection`.
 * Throw inside the responder to simulate a host-side connect-time
 * failure (the `connection-failure` / `protocol-error` / `other`
 * tagged WIT errors are the canonical shapes; see
 * `src/websocket.ts` mapToSocketError for the full mapping).
 */
export type WsResponder = (
  url: string,
  headers: [string, string][] | undefined,
) => WsMock.WebsocketConnection

/** Recorded entry per `WebsocketClient.connect(...)` call. */
export interface WsConnectCall {
  readonly url: string
  readonly headers: [string, string][] | undefined
}

export interface WsFake {
  readonly layer: Layer.Layer<WebsocketClient>
  /**
   * Override the default responder. The default responder fails
   * fast with a `connection-failure` tagged error so missing setup
   * is caught early.
   */
  readonly setResponder: (fn: WsResponder) => Effect.Effect<void>
  /**
   * Queue a one-shot failure for the *next* `connect(...)` call.
   * The supplied value is thrown verbatim (so tests can pass tagged
   * WIT error objects like `{ tag: "other", val: "..." }`). Consumed
   * exactly once on first match; subsequent calls fall through to
   * the responder.
   */
  readonly setNextConnectError: (err: unknown) => Effect.Effect<void>
  /** Snapshot the list of `(url, headers)` pairs seen so far. */
  readonly recordedConnects: Effect.Effect<ReadonlyArray<WsConnectCall>>
}

/**
 * Build a fresh fake. Use one per test — the in-memory state is
 * owned by this instance.
 */
export const make: Effect.Effect<WsFake> = Effect.sync(() => {
  let responder: WsResponder = (_url, _headers) => {
    throw {
      tag: "connection-failure",
      val: "WsFake: setResponder was not called",
    }
  }
  let nextError: Option.Option<unknown> = Option.none()
  const recorded: Array<WsConnectCall> = []

  const layer = Layer.succeed(
    WebsocketClient,
    WebsocketClient.of({
      connect: (url, headers) => {
        // Record the call up front so even failure cases are visible.
        recorded.push({ url, headers })
        if (Option.isSome(nextError)) {
          const err = nextError.value
          nextError = Option.none()
          throw err
        }
        // The mock's WebsocketConnection is structurally compatible
        // with the WIT shape exercised by `src/websocket.ts` (it
        // implements the same `send` / `receive` / `subscribe` /
        // `close` surface) but TypeScript cannot prove that the
        // hand-rolled `subscribe()` return value matches the
        // `Pollable` class brand. Cast through `unknown` to bridge
        // the nominal vs. structural gap; the runtime behaviour is
        // exercised in full by the surrounding tests.
        return responder(url, headers) as unknown as WsClient.WebsocketConnection
      },
    }),
  )

  return {
    layer,
    setResponder: (fn) =>
      Effect.sync(() => {
        responder = fn
      }),
    setNextConnectError: (err) =>
      Effect.sync(() => {
        nextError = Option.some(err)
      }),
    recordedConnects: Effect.sync(() => [...recorded]),
  }
})
