// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Plain-imperative (no Effect / Socket / Channel) fluent wrapper around
// `golem:websocket/client@1.5.0`. Ported from effect-golem's `Websocket.ts` +
// `host/WebsocketClient.ts`, reduced to a thin imperative façade over the host
// `WebsocketConnection` resource. The Effect version bridges the resource onto
// an Effect `Socket`; the fluent version exposes the host's send / receive /
// receive-with-timeout / close methods directly.

import * as WsClient from 'golem:websocket/client@1.5.0';

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const WS_TAGS = new Set<WsClient.Error['tag']>([
  'connection-failure',
  'send-failure',
  'receive-failure',
  'protocol-error',
  'closed',
  'other',
]);

interface TaggedWsError {
  readonly tag: WsClient.Error['tag'];
  readonly val?: unknown;
}

const isTaggedWsError = (e: unknown): e is TaggedWsError => {
  if (e === null || typeof e !== 'object') return false;
  const obj = e as { tag?: unknown };
  return typeof obj.tag === 'string' && WS_TAGS.has(obj.tag as TaggedWsError['tag']);
};

const extractWsError = (e: unknown): TaggedWsError | undefined => {
  if (isTaggedWsError(e)) return e;
  if (e instanceof Error) {
    const payload = (e as unknown as { payload?: unknown }).payload;
    if (isTaggedWsError(payload)) return payload;
    const cause = (e as unknown as { cause?: unknown }).cause;
    if (isTaggedWsError(cause)) return cause;
  }
  return undefined;
};

/**
 * Raised when any host call into `golem:websocket/client@1.5.0` traps. The
 * `tag` mirrors the WIT `error` variant (`connection-failure` / `send-failure`
 * / `receive-failure` / `protocol-error` / `closed` / `other`) when it can be
 * recovered from the thrown value, otherwise `undefined`. For a `closed`
 * error, {@link closeInfo} carries the host's close code / reason if present.
 */
export class WebsocketError extends Error {
  override readonly name = 'WebsocketError';
  readonly tag: WsClient.Error['tag'] | undefined;
  readonly operation: string;
  readonly closeInfo: WsClient.CloseInfo | undefined;
  constructor(
    readonly cause: unknown,
    operation: string,
  ) {
    const tagged = extractWsError(cause);
    const tag = tagged?.tag;
    const detail =
      tagged !== undefined
        ? typeof tagged.val === 'string'
          ? tagged.val
          : JSON.stringify(tagged.val ?? null)
        : cause instanceof Error
          ? cause.message
          : String(cause);
    super(`WebsocketError(${operation}${tag ? `/${tag}` : ''}): ${detail}`);
    this.tag = tag;
    this.operation = operation;
    this.closeInfo =
      tag === 'closed' ? (tagged?.val as WsClient.CloseInfo | undefined) : undefined;
  }
}

const wrap = <A>(operation: string, fn: () => A): A => {
  try {
    return fn();
  } catch (cause) {
    if (cause instanceof WebsocketError) throw cause;
    throw new WebsocketError(cause, operation);
  }
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * A received WebSocket message — text or binary, mirroring the host
 * `golem:websocket/client.message` variant.
 */
export type WebSocketMessage =
  | { readonly tag: 'text'; readonly val: string }
  | { readonly tag: 'binary'; readonly val: Uint8Array };

/** Options for {@link connectWebsocket}. */
export interface ConnectOptions {
  /**
   * Headers forwarded verbatim to the host's `connect(url, headers)` — auth
   * tokens, `Sec-WebSocket-Protocol` subprotocols, etc.
   */
  readonly headers?: ReadonlyArray<readonly [string, string]> | undefined;
}

/**
 * Handle on an open WebSocket connection. Thin imperative wrapper over the host
 * `WebsocketConnection` resource. Call {@link close} when done; the host frees
 * the resource on close.
 */
export interface WebSocketHandle {
  /** Send a text (`string`) or binary (`Uint8Array`) message. */
  send(data: string | Uint8Array): void;
  /** Receive the next message, blocking until one is available. */
  receive(): WebSocketMessage;
  /**
   * Receive the next message, waiting up to `timeoutMs` milliseconds. Returns
   * `undefined` if the timeout expires before a message arrives.
   */
  receiveWithTimeout(timeoutMs: number): WebSocketMessage | undefined;
  /** Send a close frame with an optional code and reason. */
  close(code?: number, reason?: string): void;
}

const toHostMessage = (data: string | Uint8Array): WsClient.Message =>
  typeof data === 'string' ? { tag: 'text', val: data } : { tag: 'binary', val: data };

const fromHostMessage = (msg: WsClient.Message): WebSocketMessage =>
  msg.tag === 'text' ? { tag: 'text', val: msg.val } : { tag: 'binary', val: msg.val };

const toHostHeaders = (
  headers: ReadonlyArray<readonly [string, string]> | undefined,
): [string, string][] | undefined =>
  headers === undefined ? undefined : headers.map(([k, v]) => [k, v] as [string, string]);

const makeHandle = (conn: WsClient.WebsocketConnection): WebSocketHandle => ({
  send(data) {
    wrap('send', () => conn.send(toHostMessage(data)));
  },
  receive() {
    return fromHostMessage(wrap('receive', () => conn.receive()));
  },
  receiveWithTimeout(timeoutMs) {
    const msg = wrap('receiveWithTimeout', () => conn.receiveWithTimeout(BigInt(timeoutMs)));
    return msg === undefined ? undefined : fromHostMessage(msg);
  },
  close(code, reason) {
    wrap('close', () => conn.close(code, reason));
  },
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Connect to a WebSocket server at `url` (`ws://` or `wss://`). The host's
 * `connect` is synchronous — it returns only after the handshake completes (or
 * fails), so there is no separate "open" event to await. Handshake failure
 * surfaces as {@link WebsocketError} (tag `connection-failure`).
 *
 * `connectWebsocket` is async for API ergonomics, although the host call is
 * synchronous.
 */
export async function connectWebsocket(
  url: string,
  options?: ConnectOptions,
): Promise<WebSocketHandle> {
  const conn = wrap('connect', () =>
    WsClient.WebsocketConnection.connect(url, toHostHeaders(options?.headers)),
  );
  return makeHandle(conn);
}
