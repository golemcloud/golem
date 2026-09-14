/**
 * Host service for `golem:websocket/client@1.5.0`. Wraps the WIT
 * `WebsocketConnection` resource constructor (`connect(url, headers)`)
 * so SDK code can acquire a connection via Effect-typed DI rather
 * than reaching directly into the imported namespace.
 *
 * The synchronous `connect` returns a live `WebsocketConnection`
 * resource whose `send` / `receive` / `subscribe` / `close` methods
 * are exercised directly by `src/websocket.ts`. The lifecycle
 * (`Effect.acquireRelease` calling `ws.close(1000, undefined)` on
 * scope close) lives in the consumer; the service only abstracts the
 * constructor call.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as WsClient from "golem:websocket/client@1.5.0"

export interface WebsocketClientShape {
  /**
   * Mirrors `golem:websocket/client.WebsocketConnection.connect`. The
   * call is synchronous; the host throws a tagged error
   * (`{ tag: "connection-failure", val: string }` / `protocol-error`
   * / `other`) on handshake failure. Callers wrap with
   * `Effect.try` and surface the result as a typed
   * `Socket.SocketError(SocketOpenError)`.
   */
  readonly connect: (
    url: string,
    headers: [string, string][] | undefined,
  ) => WsClient.WebsocketConnection
}

export class WebsocketClient extends Context.Service<WebsocketClient, WebsocketClientShape>()(
  "effect-golem/host/Websocket",
) {}

export const WebsocketLive: Layer.Layer<WebsocketClient> = Layer.succeed(
  WebsocketClient,
  WebsocketClient.of({
    connect: (url, headers) => WsClient.WebsocketConnection.connect(url, headers),
  }),
)
