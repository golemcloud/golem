// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// Create a WebSocket tunnel from an existing HTTP client.
/// The HTTP client must be in a clean state (i.e. not in the middle of a request).
/// A WebSocket handshake will be sent to `path` via the HTTP client.
/// If the handshake succeeds, a WebSocket tunnel will be established and returned.
///
/// The ownership of the HTTP client will be transferred to this function.
/// So caller must not use the client anymore, nor close it.
///
/// Extra headers during the WebSocket handshake can be specified via `extra_headers`.
/// Some headers are reserved for the WebSocket protocol
/// and must NOT be set in `extra_headers`:
///
/// - those headers set by `@http.Client`, see `@http.Client::new`
/// - `Connection`, `Upgrade`
/// - `Sec-WebSocket-Key`, `Sec-WebSocket,Version`
#as_free_fn
#warnings("-alert_internal")
pub async fn Conn::from_http_client(
  conn : @http.Client,
  path : StringView,
  extra_headers? : @http.Headers = Map([]),
  required_subprotocol? : String,
) -> Conn {
  // Ref : https://datatracker.ietf.org/doc/html/rfc6455#section-4.1
  let seed = @tls.rand_bytes(32)
  let rand = @random.Rand::chacha8(seed~)

  // Send WebSocket handshake request
  let nonce = FixedArray::make(16, b'\x00')
  nonce.unsafe_write_uint32_le(0, rand.uint())
  nonce.unsafe_write_uint32_le(4, rand.uint())
  nonce.unsafe_write_uint32_le(8, rand.uint())
  nonce.unsafe_write_uint32_le(12, rand.uint())
  let key = base64_encode(nonce.unsafe_reinterpret_as_bytes())
  extra_headers["Connection"] = "Upgrade"
  extra_headers["Upgrade"] = "websocket"
  extra_headers["Sec-WebSocket-Key"] = key
  extra_headers["Sec-WebSocket-Version"] = "13"
  errdefer conn.close()
  let response = conn..request(Get, path, extra_headers~).end_request()
  guard response.code is 101 else {
    raise HandshakeRejected(
      "Server did not respond with 101 Switching Protocols: \{response.code} \{response.reason}",
      response,
    )
  }

  // Validate WebSocket handshake headers
  guard response.headers.get("upgrade") is Some(upgrade) &&
    upgrade.to_lower() == "websocket" else {
    raise HandshakeRejected("Missing or invalid Upgrade header", response)
  }
  guard response.headers.get("connection") is Some(connection) &&
    connection.to_lower().contains("upgrade") else {
    raise HandshakeRejected("Missing or invalid Connection header", response)
  }
  guard response.headers.get("sec-websocket-accept") is Some(accept_key) else {
    raise HandshakeRejected("Missing Sec-WebSocket-Accept header", response)
  }
  let expected_accept_key = generate_accept_key(key)
  guard accept_key == expected_accept_key else {
    raise HandshakeRejected(
      "Invalid Sec-WebSocket-Accept value: \{accept_key} != \{expected_accept_key}",
      response,
    )
  }
  match required_subprotocol {
    Some(required) => {
      guard selected_subprotocol_is_valid(response.headers, required) else {
        raise HandshakeRejected(
          "Server did not select required WebSocket subprotocol", response,
        )
      }
    }
    None => ()
  }
  conn.enter_passthrough_mode()
  Conn::new(conn, 4096, mask=Some(rand))
}

///|
fn selected_subprotocol_is_valid(
  response_headers : @http.Headers,
  required : String,
) -> Bool {
  response_headers.get("sec-websocket-protocol") == Some(required)
}

///|
/// Connect to a WebSocket server via the given URL.
/// The protocol of the URL must be either `ws` (for unencrypted WebSocket connection)
/// or `wss` (for TLS encrypted WebSocket connection).
///
/// Extra headers during the WebSocket handshake can be specified via `extra_headers`.
/// Some headers are reserved for the WebSocket protocol
/// and must NOT be set in `extra_headers`:
///
/// - those headers set by `@http.Client`, see `@http.Client::new`
/// - `Connection`, `Upgrade`
/// - `Sec-WebSocket-Key`, `Sec-WebSocket,Version`
///
/// If `proxy` is present, the websocket client will tunnel traffic through the proxy.
/// See `@http.Client::new` for more details.
///
/// Example:
/// ```moonbit no-check
/// let ws = Client::connect("ws://example.com/endpoint")
/// ```
#as_free_fn
pub async fn Conn::connect(
  url : String,
  headers? : @http.Headers = Map([]),
  proxy? : @http.Client,
  required_subprotocol? : String,
) -> Conn {
  guard url.find("://") is Some(protocol_len) else {
    abort("`@websocket.Client::connect()`: invalid URL: missing protocol")
  }
  let http_protocol = match url[:protocol_len] {
    "ws" => "http"
    "wss" => "https"
    protocol =>
      abort("`@websocket.Client::connect()`: invalid protocol \{protocol}")
  }
  let url = url[protocol_len + 3:]
  let (host, path) = if url.find("/") is Some(i) {
    (url[:i], url[i:])
  } else {
    (url, "/")
  }
  let path : StringView = if path == "" { "/" } else { path }
  let conn = @http.Client("\{http_protocol}://\{host}", proxy?)
  Conn::from_http_client(
    conn,
    path,
    extra_headers=headers,
    required_subprotocol?,
  )
}
