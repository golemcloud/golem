///|
/// What a connection does after runtime reconstruction (suspension, restart
/// or crash), when its physical session is gone.
pub(all) enum ReconstructionPolicy {
  /// Opens a replacement connection with the same URL and headers on the
  /// first live access. Choose it only when the peer accepts a new session in
  /// place of the lost one.
  ReconnectAutomatically
  /// The first live access raises `WebsocketError::SessionLost`, and the
  /// connection stays lost. This is the host default, used when `connect`
  /// gets no policy.
  ReportConnectionLoss
} derive(Debug, Eq)

///|
/// A WebSocket message: text or binary.
pub(all) enum Message {
  Text(String)
  Binary(Bytes)
} derive(Debug, Eq)

///|
/// The `golem:websocket/client` error variant.
pub(all) suberror WebsocketError {
  ConnectionFailure(String)
  SendFailure(String)
  ReceiveFailure(String)
  ProtocolError(String)
  /// The peer closed the connection, with its close code and reason if any.
  Closed((UInt32, String)?)
  Other(String)
  /// The physical session did not survive runtime reconstruction of a
  /// connection that reports connection loss. An earlier send may still have
  /// reached the peer.
  SessionLost
} derive(Debug, Eq)

///|
/// An open outbound WebSocket connection.
pub struct WebsocketConnection {
  priv raw : @client.WebsocketConnection
}

///|
/// Connects to `url` (`ws://` or `wss://`).
///
/// Without `reconstruction_policy`, no policy is sent and the host default
/// `ReportConnectionLoss` applies: after runtime reconstruction the next
/// operation raises `WebsocketError::SessionLost` and no replacement
/// connection is opened.
pub fn connect(
  url : String,
  headers? : Array[(String, String)],
  reconstruction_policy? : ReconstructionPolicy,
) -> WebsocketConnection raise WebsocketError {
  let policy = reconstruction_policy.map(policy => {
    match policy {
      ReconnectAutomatically =>
        @client.ReconstructionPolicy::RECONNECT_AUTOMATICALLY
      ReportConnectionLoss =>
        @client.ReconstructionPolicy::REPORT_CONNECTION_LOSS
    }
  })
  match @client.WebsocketConnection::connect(url, headers, policy) {
    Ok(raw) => { raw, }
    Err(error) => raise from_host(error)
  }
}

///|
/// Sends a text or binary frame.
pub fn WebsocketConnection::send(
  self : WebsocketConnection,
  message : Message,
) -> Unit raise WebsocketError {
  let message = match message {
    Text(text) => @client.Message::Text(text)
    Binary(bytes) =>
      @client.Message::Binary(FixedArray::from_iter(bytes.iter()))
  }
  match self.raw.send(message) {
    Ok(_) => ()
    Err(error) => raise from_host(error)
  }
}

///|
/// Receives the next message, waiting until one is available. Host failures
/// raise `WebsocketError`; cancellation of the current task propagates as is.
pub async fn WebsocketConnection::receive(
  self : WebsocketConnection,
) -> Message {
  match self.raw.receive() {
    Ok(message) => to_message(message)
    Err(error) => raise from_host(error)
  }
}

///|
/// Receives the next message, waiting up to `timeout_ms` milliseconds. `None`
/// means the timeout expired first.
pub async fn WebsocketConnection::receive_with_timeout(
  self : WebsocketConnection,
  timeout_ms : UInt64,
) -> Message? {
  match self.raw.receive_with_timeout(timeout_ms) {
    Ok(message) => message.map(to_message)
    Err(error) => raise from_host(error)
  }
}

///|
/// Sends a close frame with an optional code and reason.
pub fn WebsocketConnection::close(
  self : WebsocketConnection,
  code? : UInt32,
  reason? : String,
) -> Unit raise WebsocketError {
  match self.raw.close(code, reason) {
    Ok(_) => ()
    Err(error) => raise from_host(error)
  }
}

///|
/// Releases the resource locally without a close frame.
pub fn WebsocketConnection::drop(self : WebsocketConnection) -> Unit {
  self.raw.drop()
}

///|
fn to_message(message : @client.Message) -> Message {
  match message {
    Text(text) => Text(text)
    Binary(bytes) => Binary(Bytes::from_array(bytes))
  }
}

///|
fn from_host(error : @client.Error_) -> WebsocketError {
  match error {
    ConnectionFailure(message) => ConnectionFailure(message)
    SendFailure(message) => SendFailure(message)
    ReceiveFailure(message) => ReceiveFailure(message)
    ProtocolError(message) => ProtocolError(message)
    Closed(info) => Closed(info.map(info => (info.code, info.reason)))
    Other(message) => Other(message)
    SessionLost => SessionLost
  }
}
