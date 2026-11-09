# WebSocket client

> A WebSocket client over `golem:websocket/client@1.5.0`. **Status:** ✅ Complete for the 1.6.0 API. Verified on a real 1.6.0 executor against a local
> echo server (`scripts/websocket/WebSocketAgents.kt`).

Types live in `cloud.golem.websocket` (`WebSocketConnection`, `WebSocketMessage`, `WebSocketError`, `WebSocketException`, `ReconstructionPolicy`).

```kotlin
val c = WebSocketConnection.connect(
    "wss://example.com/socket",
    headers = listOf("authorization" to "Bearer ..."),
    reconstructionPolicy = ReconstructionPolicy.REPORT_CONNECTION_LOSS,
)
c.send("hello")                                   // or c.send(WebSocketMessage.Binary(bytes))
when (val m = c.receive()) {                      // blocks for the next message
    is WebSocketMessage.Text -> println(m.text)
    is WebSocketMessage.Binary -> println(m.bytes.size)
}
val maybe = c.receive(timeoutMs = 500)            // null if nothing arrived in time
c.close(1000, "done")
```

- `WebSocketConnection.connect(url, headers?, reconstructionPolicy?)` throws `WebSocketException` (with the decoded `WebSocketError`) if the connection fails.
- `send` / `receive` / `receive(timeoutMs)` / `close` block; every call is recorded durably, so a restarted agent sees the same messages.
- `ReconstructionPolicy`: `RECONNECT_AUTOMATICALLY` (the runtime's default) reconnects after a restart; `REPORT_CONNECTION_LOSS` surfaces the loss to the agent as
  `WebSocketError.SessionLost`. A close by the peer arrives as `WebSocketError.Closed(code, reason)`.
- `WebSocketError`: `ConnectionFailure`, `SendFailure`, `ReceiveFailure`, `ProtocolError`, `Closed`, `Other`, `SessionLost`.
- `WebSocketException.error` carries the `WebSocketError`; `WebSocketMessage.Text(text)` / `Binary(bytes)`; `Closed(code: Int?, reason: String?)`.
- `close(code?, reason?)` sends a close frame and releases the handle; `release()` drops the handle without one. Handles are not reclaimed by Kotlin/Wasm GC.

Verified: request headers reach the server, text and binary echo, `receive(timeout)` returns `null` when idle, a failed connect is `ConnectionFailure`. Not exercised: session loss and
automatic reconnection across an agent restart (the harness has dedicated tests for the host side).

See also: [Host API](host-api.md) · [SDK README](../../README.md).
