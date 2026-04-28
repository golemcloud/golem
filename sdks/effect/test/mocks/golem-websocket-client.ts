/**
 * Runtime mock for `golem:websocket/client@1.5.0`. Implements the
 * slice of the host API actually exercised by `src/websocket.ts` and
 * its unit tests:
 *
 * - `WebsocketConnection.connect(url, headers)` — by default fails
 *   with the `connection-failure` tagged error so tests have to opt
 *   in to a mock connection via `__setConnectImpl(...)`.
 * - The returned mock connection serializes outbound messages via
 *   `__pushOutbound` / `__takeOutbound` and inbound messages via
 *   `__deliverInbound` / `__deliverError` / `__signalClosed`.
 *
 * Inbound delivery is wired through a JS `Promise` queue so it
 * mirrors the real host's `subscribe()` -> `pollable.promise()` ->
 * `receive()` round-trip exactly: `subscribe()` returns an object
 * with a `.promise()` that resolves when the next inbound frame is
 * available, and `receive()` then returns the buffered message
 * (or throws the buffered error / `closed` envelope).
 *
 * State is reset via `__resetAll()`.
 */

export type Message = { tag: "text"; val: string } | { tag: "binary"; val: Uint8Array }

export type CloseInfo = {
  code: number
  reason: string
}

export type WsError =
  | { tag: "connection-failure"; val: string }
  | { tag: "send-failure"; val: string }
  | { tag: "receive-failure"; val: string }
  | { tag: "protocol-error"; val: string }
  | { tag: "closed"; val: CloseInfo | undefined }
  | { tag: "other"; val: string }

interface PendingFrame {
  readonly kind: "msg" | "err"
  readonly msg?: Message
  readonly err?: WsError
}

export class WebsocketConnection {
  /** @internal */
  readonly _url: string
  /** @internal */
  readonly _headers: ReadonlyArray<readonly [string, string]> | undefined
  /** @internal */
  _closed = false
  /** @internal */
  _outbound: Message[] = []
  /** @internal */
  _outboundCloses: { code?: number; reason?: string }[] = []
  /** @internal */
  _inbound: PendingFrame[] = []
  /** @internal */
  _waiters: Array<() => void> = []
  /** @internal */
  _sendImpl: ((msg: Message) => void) | undefined

  constructor(url: string, headers: ReadonlyArray<readonly [string, string]> | undefined) {
    this._url = url
    this._headers = headers
  }

  static connect(url: string, headers: [string, string][] | undefined): WebsocketConnection {
    return _connectImpl(url, headers)
  }

  send(message: Message): void {
    if (this._closed) {
      throw { tag: "send-failure", val: "connection is closed" } as WsError
    }
    if (this._sendImpl) this._sendImpl(message)
    this._outbound.push(message)
  }

  receive(): Message {
    const frame = this._inbound.shift()
    if (!frame) {
      throw {
        tag: "receive-failure",
        val: "test invariant: receive() called with no buffered frame",
      } as WsError
    }
    if (frame.kind === "err") {
      throw frame.err
    }
    return frame.msg!
  }

  receiveWithTimeout(_timeoutMs: bigint): Message | undefined {
    const frame = this._inbound.shift()
    if (!frame) return undefined
    if (frame.kind === "err") throw frame.err
    return frame.msg
  }

  close(code: number | undefined, reason: string | undefined): void {
    if (this._closed) return
    this._closed = true
    this._outboundCloses.push({ code, reason })
    // Schedule a `closed` error for the next receive() so the read
    // loop in src/websocket.ts terminates with a SocketCloseError.
    this._inbound.push({
      kind: "err",
      err: {
        tag: "closed",
        val: { code: code ?? 1000, reason: reason ?? "" },
      },
    })
    this._wake()
  }

  subscribe(): {
    promise(): Promise<void>
    abortablePromise(signal: AbortSignal): Promise<void>
  } {
    const inbound = this._inbound
    const waiters = this._waiters
    return {
      promise: () =>
        new Promise<void>((resolve) => {
          if (inbound.length > 0) {
            resolve()
            return
          }
          waiters.push(resolve)
        }),
      abortablePromise: (signal: AbortSignal) =>
        new Promise<void>((resolve, reject) => {
          if (signal.aborted) {
            reject(new DOMException("Aborted", "AbortError"))
            return
          }
          if (inbound.length > 0) {
            resolve()
            return
          }
          let settled = false
          const onAbort = () => {
            if (settled) return
            settled = true
            const idx = waiters.indexOf(waiter)
            if (idx >= 0) waiters.splice(idx, 1)
            reject(new DOMException("Aborted", "AbortError"))
          }
          const waiter = () => {
            if (settled) return
            settled = true
            signal.removeEventListener("abort", onAbort)
            resolve()
          }
          waiters.push(waiter)
          signal.addEventListener("abort", onAbort, { once: true })
        }),
    }
  }

  /** @internal */
  _wake(): void {
    if (this._inbound.length === 0) return
    const w = this._waiters.splice(0)
    for (const f of w) f()
  }
}

let _connectImpl: (url: string, headers: [string, string][] | undefined) => WebsocketConnection = (
  _url,
  _headers,
) => {
  // Default: simulate a connection failure if the test hasn't set up
  // a stub. This catches "I forgot to mock the host" footguns early.
  throw {
    tag: "connection-failure",
    val: "test default: __setConnectImpl was not called",
  } as WsError
}

/**
 * Override `WebsocketConnection.connect` for the duration of a test.
 * Most tests will return a fresh `WebsocketConnection(url, headers)`.
 */
export const __setConnectImpl = (
  fn: (url: string, headers: [string, string][] | undefined) => WebsocketConnection,
): void => {
  _connectImpl = fn
}

/** Default connect impl: returns a fresh, unwired WebsocketConnection. */
export const __defaultConnect = (
  url: string,
  headers: [string, string][] | undefined,
): WebsocketConnection => new WebsocketConnection(url, headers ?? undefined)

/**
 * Push a text/binary message into the connection's inbound queue. The
 * read loop (subscribe -> receive) will see it on its next pass.
 */
export const __deliverInbound = (conn: WebsocketConnection, msg: Message): void => {
  conn._inbound.push({ kind: "msg", msg })
  conn._wake()
}

/** Push a tagged error into the connection's inbound queue. */
export const __deliverError = (conn: WebsocketConnection, err: WsError): void => {
  conn._inbound.push({ kind: "err", err })
  conn._wake()
}

/** Convenience: push a `closed` error. */
export const __signalClosed = (conn: WebsocketConnection, info: CloseInfo): void => {
  __deliverError(conn, { tag: "closed", val: info })
}

/** Snapshot of all messages the SUT has sent so far. */
export const __outbound = (conn: WebsocketConnection): ReadonlyArray<Message> => [...conn._outbound]

/** Snapshot of all close() calls the SUT has issued so far. */
export const __outboundCloses = (
  conn: WebsocketConnection,
): ReadonlyArray<{ code?: number; reason?: string }> => [...conn._outboundCloses]

export const __resetAll = (): void => {
  _connectImpl = (_url, _headers) => {
    throw {
      tag: "connection-failure",
      val: "test default: __setConnectImpl was not called",
    } as WsError
  }
}

export const __reset = __resetAll
