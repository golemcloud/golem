// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
const STREAM_SUBPROTOCOL : String = "golem.agent-invocation.v1"

///|
const MAX_MESSAGE_BYTES : Int32 = 32 * 1024 * 1024

///|
const MAX_BINARY_METADATA_BYTES : Int32 = 16 * 1024

///|
const MAX_PACKED_U8_BYTES : Int32 = 1024 * 1024

///|
const MAX_LOGICAL_VALUE_BYTES : Int32 = 16 * 1024 * 1024

///|
const MAX_QUEUED_OUTPUT_ITEMS : Int32 = 256

///|
const MAX_QUEUED_OUTPUT_BYTES : Int32 = 16 * 1024 * 1024

///|
const MAX_INPUT_REPLAY_BYTES : Int32 = 16 * 1024 * 1024

///|
const U64_MAX : UInt64 = 18446744073709551615UL

///|
pub(all) enum PendingInputPayload {
  PendingJson(SchemaValue)
  PendingU8(Bytes)
  PendingBinary(Bytes, String?)
  PendingEnd(Json)
}

///|
pub(all) struct PendingInput {
  mut start : UInt64
  mut count : UInt64
  mut payload : PendingInputPayload
  mut bytes : Int32
}

///|
/// Owns an admitted producer diagnostic until the pending terminal takes it.
/// The replay budget exists before attachment and is shared with the session.
pub struct InputFailure {
  mut diagnostic : String
  mut bytes : Int32
  mut rejected : Bool
  mut disposed : Bool
  replay_bytes : Ref[Int32]
}

///|
pub(all) struct InputRegistration {
  mut id : String
  next : async () -> SchemaValue?
  try_next : () -> SchemaValue?? raise
  cancel : async () -> Unit
  wire_kind : String
  codec : PublicValueCodec
  mut stream_token : String?
  mut channel : UInt32?
  mut sequence : UInt64
  mut terminal : Bool
  mut source_ended : Bool
  mut source_failed : Bool
  failure : InputFailure
  mut pending : PendingInput?
}

///|
pub struct StreamEncodeContext {
  inputs : Array[InputRegistration]
  replay_bytes : Ref[Int32]
}

///|
pub fn StreamEncodeContext::new() -> StreamEncodeContext {
  { inputs: [], replay_bytes: Ref(0), }
}

///|
priv struct OutputBatch {
  values : Array[SchemaValue]
  mut index : Int32
  cursor : String?
  error : BridgeError?
  bytes : Int32
  next_sequence : UInt64
  terminal : Bool
}

///|
#warnings("-unused_field")
priv struct LiveStreamSession {
  mut socket : @websocket.Conn
  url : String
  headers : @http.Headers
  mut session_token : String?
  mut attempt_id : String
  agent_id : AgentId
  idempotency_key : String
  inputs : Array[InputRegistration]
  input_channels : Map[String, UInt32]
  input_tokens : Map[String, InputRegistration]
  mut next_input_index : Int32
  input_replay_bytes : Ref[Int32]
  output_channels : Map[String, UInt32]
  output_sequences : Map[String, UInt64]
  output_queues : Map[String, @aqueue.Queue[OutputBatch]]
  output_active : Map[String, OutputBatch]
  output_lanes : Map[String, String]
  output_codecs : Map[String, PublicValueCodec]
  result_codec : PublicValueCodec
  output_terminals : Map[String, Bool]
  output_claimed : Map[String, Bool]
  output_cancelled : Map[String, Bool]
  queue_bytes : Map[String, Int32]
  mut total_queue_bytes : Int32
  cursors : Map[String, String]
  cursor_sequences : Map[String, UInt64]
  cursor_terminals : Map[String, Bool]
  result_queue : @aqueue.Queue[AgentInvocationResult]
  socket_write : @async.Mutex
  output_space : @cond.Cond
  mut result_seen : Bool
  mut finished : Bool
  mut actor_owner : @io.MemoryReader?
  clock : ReconnectClock
}

///|
async fn connect_socket(
  url : String,
  headers : @http.Headers,
) -> @websocket.Conn {
  @websocket.connect(
    url,
    headers=headers.copy(),
    required_subprotocol=STREAM_SUBPROTOCOL,
  )
}

///|
const FIRST_RECONNECT_DELAY_MS : Int32 = 50

///|
const MAX_RECONNECT_DELAY_MS : Int32 = 3000

///|
/// Bounds one connection attempt: the TCP connect, the TLS handshake and the
/// WebSocket upgrade together.
let connect_attempt_timeout_ms : Ref[Int32] = { val: 10000, }

///|
/// How long a connection must stay up after the server accepted its operation
/// for its loss to count as progress.
let stable_connection_ms : Ref[Int32] = { val: 5000, }

///|
/// Paces reconnects and ends them once the reconnect window passes without
/// progress.
priv struct ReconnectClock {
  policy : ReconnectPolicy
  endpoint : String
  /// When the call last made progress: its start, or when it lost a stable
  /// connection.
  mut progress_at : Int64
  /// When the server accepted the operation of the current connection.
  mut accepted_at : Int64?
  /// The delay before the next attempt; zero makes the next attempt at once.
  mut delay_ms : Int32
  mut last_cause : String
}

///|
fn ReconnectClock::new(
  policy : ReconnectPolicy,
  endpoint : String,
) -> ReconnectClock {
  {
    policy,
    endpoint,
    progress_at: @async.now(),
    accepted_at: None,
    delay_ms: 0,
    last_cause: "connection closed",
  }
}

///|
/// Records that the server accepted the operation of the current connection.
fn ReconnectClock::accepted(self : ReconnectClock) -> Unit {
  self.accepted_at = Some(@async.now())
}

///|
/// Leaves the current connection, and reports whether it was stable: up for
/// the stable time after the server accepted its operation. Losing a stable
/// connection is progress: it restarts the window and makes the next attempt
/// at once.
fn ReconnectClock::left_connection(self : ReconnectClock) -> Bool {
  let stable = match self.accepted_at {
    Some(at) => (@async.now() - at).to_int() >= stable_connection_ms.val
    None => false
  }
  self.accepted_at = None
  if stable {
    self.progress_at = @async.now()
    self.delay_ms = 0
  }
  stable
}

///|
/// Records a failed attempt; the next one waits longer.
fn ReconnectClock::failed(self : ReconnectClock, cause : String) -> Unit {
  self.last_cause = cause
  self.delay_ms = if self.delay_ms == 0 {
    FIRST_RECONNECT_DELAY_MS
  } else if self.delay_ms * 2 > MAX_RECONNECT_DELAY_MS {
    MAX_RECONNECT_DELAY_MS
  } else {
    self.delay_ms * 2
  }
}

///|
fn ReconnectClock::remaining(self : ReconnectClock) -> Int32? {
  match self.policy {
    Unlimited => None
    Within(window_ms) =>
      Some(window_ms - (@async.now() - self.progress_at).to_int())
  }
}

///|
/// Waits for the next attempt and returns how long that attempt may take;
/// raises [BridgeError::Unreachable] once the window has passed.
async fn ReconnectClock::next_attempt(self : ReconnectClock) -> Int32 {
  if self.delay_ms > 0 {
    let delay = match self.remaining() {
      None => self.delay_ms
      Some(left) if left < self.delay_ms => left
      Some(_) => self.delay_ms
    }
    if delay > 0 {
      @async.sleep(delay)
    }
  }
  match (self.policy, self.remaining()) {
    (Within(window_ms), Some(left)) if left <= 0 =>
      raise Unreachable(
        endpoint=self.endpoint,
        window_ms~,
        last_cause=self.last_cause,
      )
    (_, Some(left)) if left < connect_attempt_timeout_ms.val => left
    _ => connect_attempt_timeout_ms.val
  }
}

///|
/// One attempt to attach with `descriptor`: connect within `timeout_ms`, then
/// send the descriptor and read the server's answer until the reconnect window
/// of `clock` ends. The window also bounds the send, as a server that stops
/// reading leaves it pending. `Err` is a failure worth another attempt with the same
/// descriptor: a lost connection, an upgrade refused with HTTP 429 or 5xx, or
/// an answer that did not come within the window. Any other failure is raised.
async fn attach_attempt(
  clock : ReconnectClock,
  url : String,
  headers : @http.Headers,
  descriptor : String,
  timeout_ms : Int32,
) -> Result[(@websocket.Conn, Map[String, Json]), String] {
  try {
    guard @async.with_timeout_opt(timeout_ms, () => connect_socket(url, headers))
      is Some(socket) else {
      return Err("no WebSocket connection within \{timeout_ms} ms")
    }
    let answer = match clock.remaining() {
      None => {
        socket.send_text(descriptor)
        Some(read_text(socket))
      }
      Some(left) =>
        @async.with_timeout_opt(if left > 0 { left } else { 1 }, () => {
          socket.send_text(descriptor)
          read_text(socket)
        })
    }
    guard answer is Some(answer) else {
      socket.close()
      return Err("the server answered neither the start nor the resume")
    }
    Ok((socket, expect_object(answer)))
  } catch {
    @websocket.HandshakeRejected(message, response) =>
      if response.code == 429 || response.code >= 500 {
        Err(message)
      } else {
        raise BridgeError(message)
      }
    @websocket.ConnectionClosed(_, _) => Err("connection closed")
    @io.ReaderClosed => Err("connection closed")
    @os_error.OSError(_) as error => Err("connection failed: \{error}")
    error => raise error
  }
}

///|
/// Replaces each provisional stream reference in `value` that `renamed` maps.
fn replace_provisional_refs(
  value : Json,
  renamed : Map[String, String],
) -> Json {
  match value {
    Array(items) =>
      Json::array(items.map(item => replace_provisional_refs(item, renamed)))
    Object(fields) => {
      if fields.get("kind") is Some(String("stream")) &&
        fields.get("value") is Some(Object(reference)) &&
        reference.get("provisionalRef") is Some(String(previous)) &&
        renamed.get(previous) is Some(renewed) {
        let renewed_reference : Map[String, Json] = Map([])
        reference.each((name, field) => {
          renewed_reference[name] = if name == "provisionalRef" {
            Json::string(renewed)
          } else {
            field
          }
        })
        let renewed_stream : Map[String, Json] = Map([])
        fields.each((name, field) => {
          renewed_stream[name] = if name == "value" {
            Json::object(renewed_reference)
          } else {
            field
          }
        })
        return Json::object(renewed_stream)
      }
      let copy : Map[String, Json] = Map([])
      fields.each((name, field) => {
        copy[name] = replace_provisional_refs(field, renamed)
      })
      Json::object(copy)
    }
    other => other
  }
}

///|
/// Gives every stream argument a new provisional reference, in `inputs` and in
/// the start's method `parameters`: only an exact retry may reuse one.
fn renew_provisional_refs(
  parameters : Json,
  inputs : Array[InputRegistration],
) -> Json {
  let renamed : Map[String, String] = Map([])
  for input in inputs {
    let renewed = fresh_id()
    renamed[input.id] = renewed
    input.id = renewed
  }
  replace_provisional_refs(parameters, renamed)
}

///|
/// Sends the start until the server answers it: the same frozen start again
/// after a lost connection, and a new attempt with new provisional references
/// after a retryable rejection. Returns the socket, the acceptance and the
/// accepted attempt.
async fn start_invocation(
  clock : ReconnectClock,
  url : String,
  headers : @http.Headers,
  inputs : Array[InputRegistration],
  method_parameters : Json,
  freeze : (String, Json) -> String,
) -> (@websocket.Conn, Map[String, Json], String) {
  let mut attempt_id = fresh_id()
  let mut parameters = method_parameters
  let mut frozen_start = freeze(attempt_id, parameters)
  for ;; {
    let timeout_ms = clock.next_attempt()
    match attach_attempt(clock, url, headers, frozen_start, timeout_ms) {
      Err(cause) => clock.failed(cause)
      Ok((socket, response)) =>
        match expect_string(get_field(response, "type")) {
          "invocationAccepted" => {
            clock.accepted()
            return (socket, response, attempt_id)
          }
          "invocationRejected" => {
            socket.close()
            match field_opt(response, "attemptId") {
              Some(value) if expect_string(value) != attempt_id =>
                raise BridgeError(
                  "attempt-conflict: rejection attempt does not match",
                )
              _ => ()
            }
            let code = expect_string(get_field(response, "code"))
            let message = expect_string(get_field(response, "message"))
            if !expect_bool(get_field(response, "retryable")) ||
              code == "invalid-attachment-state" {
              raise reported_failure(response)
            }
            clock.failed("\{code}: \{message}")
            attempt_id = fresh_id()
            parameters = renew_provisional_refs(parameters, inputs)
            frozen_start = freeze(attempt_id, parameters)
          }
          _ => raise BridgeError("protocol-error: invalid start response")
        }
    }
  }
}

///|
async fn send_session_text(session : LiveStreamSession, text : String) -> Unit {
  session.socket_write.acquire()
  defer session.socket_write.release()
  session.socket.send_text(text)
}

///|
async fn send_session_binary(
  session : LiveStreamSession,
  bytes : Bytes,
) -> Unit {
  session.socket_write.acquire()
  defer session.socket_write.release()
  session.socket.send_binary(bytes)
}

///|
/// Resumes a call the server had accepted, after its connection was lost, its
/// attachment revoked, or its attempt rejected with the retryable `rejection`.
/// Losing a stable connection restarts the reconnect window; losing it sooner
/// counts as a failed attempt.
async fn resume_session(
  session : LiveStreamSession,
  rejection? : String,
) -> Unit {
  let stable = session.clock.left_connection()
  match rejection {
    Some(cause) => session.clock.failed(cause)
    None => if !stable { session.clock.failed("connection closed") }
  }
  guard session.session_token is Some(token) else {
    raise BridgeError("invalid-attachment-state: accepted session has no token")
  }
  let cursors : Array[Json] = []
  session.cursors.each((_, cursor) => cursors.push(Json::string(cursor)))
  let mut attempt_id = fresh_id()
  let mut takeover = false
  let mut frozen_resume = protocol_message("resumeAttach", [
    ("attemptId", Json::string(attempt_id)),
    ("operation", Json::string("resume")),
    ("outputCursors", Json::array(cursors)),
    ("sessionToken", Json::string(token)),
  ])
  let mut accepted : Map[String, Json]? = None
  let mut socket : @websocket.Conn? = None
  while accepted is None {
    let timeout_ms = session.clock.next_attempt()
    match
      attach_attempt(
        session.clock,
        session.url,
        session.headers,
        frozen_resume,
        timeout_ms,
      ) {
      Err(cause) => session.clock.failed(cause)
      Ok((candidate, response)) =>
        match expect_string(get_field(response, "type")) {
          "invocationAccepted" => {
            accepted = Some(response)
            socket = Some(candidate)
          }
          "invocationRejected" => {
            candidate.close()
            if expect_string(get_field(response, "attemptId")) != attempt_id {
              raise BridgeError(
                "attempt-conflict: rejection attempt does not match",
              )
            }
            let code = expect_string(get_field(response, "code"))
            let message = expect_string(get_field(response, "message"))
            let retryable = expect_bool(get_field(response, "retryable"))
            // A resume refused because the old attachment is still live is
            // retried once as a takeover; other retryable rejections are
            // retried as they are.
            if !((!takeover && code == "invalid-attachment-state") ||
              (retryable && code != "invalid-attachment-state")) {
              raise reported_failure(response)
            }
            session.clock.failed("\{code}: \{message}")
            takeover = takeover || code == "invalid-attachment-state"
            attempt_id = fresh_id()
            frozen_resume = protocol_message("resumeAttach", [
              ("attemptId", Json::string(attempt_id)),
              (
                "operation",
                Json::string(if takeover { "takeover" } else { "resume" }),
              ),
              ("outputCursors", Json::array(cursors)),
              ("sessionToken", Json::string(token)),
            ])
          }
          _ =>
            raise BridgeError(
              "invalid-attachment-state: invalid resume response",
            )
        }
    }
  }
  let accepted = accepted.unwrap()
  let socket = socket.unwrap()
  ignore(
    validate_acceptance_identity(
      accepted,
      attempt_id,
      Some((session.agent_id, session.idempotency_key)),
    ),
  )
  session.clock.accepted()
  session.socket_write.acquire()
  session.socket = socket
  session.socket_write.release()
  session.attempt_id = attempt_id
  session.input_channels.clear()
  session.output_channels.clear()
  for input in session.inputs {
    input.channel = None
  }
  session.output_queues.each((stream_token, queue) => {
    drain_output_queue(session, stream_token, queue)
  })
  session.output_sequences.each((token, _) => {
    session.output_sequences[token] = session.cursor_sequences
      .get(token)
      .unwrap_or(0UL)
    session.output_terminals[token] = session.cursor_terminals
      .get(token)
      .unwrap_or(false)
  })
  install_mappings(session, accepted)
  for input in session.inputs {
    if input.stream_token is Some(_) && input.channel is None {
      raise BridgeError(
        "invalid-attachment-state: resume omitted an input mapping",
      )
    }
  }
  session.output_queues.each((token, _) => {
    if !session.output_channels.contains(token) {
      raise BridgeError(
        "invalid-attachment-state: resume omitted an output mapping",
      )
    }
  })
  session.session_token = Some(
    expect_string(get_field(accepted, "sessionToken")),
  )
  for input in session.inputs {
    match input.pending {
      Some(pending) => send_pending_input(session, input, pending)
      None => ()
    }
  }
  pump_next_input(session)
}

///|
let stream_sessions : Map[String, LiveStreamSession] = Map([])

///|
fn fresh_id() -> String {
  Uuid::random().to_string()
}

///|
pub fn[T] register_input_stream(
  context : StreamEncodeContext,
  stream : AgentStream[T],
  encode : async (T) -> SchemaValue,
  codec : PublicValueCodec,
  wire_kind~ : String = "json",
) -> SchemaValue raise {
  stream.acquire()
  let id = fresh_id()
  let failure : InputFailure = {
    diagnostic: "",
    bytes: 0,
    rejected: false,
    disposed: false,
    replay_bytes: context.replay_bytes,
  }
  let mut next = async fn() {
    match stream.next_input() {
      Some(value) => Some(encode(value))
      None => None
    }
  }
  let mut try_next : () -> SchemaValue?? raise = fn() { None }
  let mut cancel = async fn() { stream.cancel_input() }
  if wire_kind == "u8" {
    let prefetched : @aqueue.Queue[SchemaValue?] = @aqueue.Queue(
      kind=@aqueue.Blocking(4096),
    )
    let owner = @io.MemoryReader(async fn(_) {
      for ;; {
        let value = try stream.next_input() catch {
          error => {
            failure.rejected = !admit_input_failure(failure, error)
            prefetched.close()
            return
          }
        } noraise {
          value => value
        }
        match value {
          Some(value) => {
            let encoded = try encode(value) catch {
              error => {
                failure.rejected = !admit_input_failure(failure, error)
                prefetched.close()
                return
              }
            } noraise {
              value => value
            }
            prefetched.put(Some(encoded))
          }
          None => {
            prefetched.put(None)
            return
          }
        }
      }
    })
    next = async fn() { prefetched.get() }
    try_next = fn() raise { prefetched.try_get() }
    cancel = async fn() {
      failure.disposed = true
      owner.close()
      prefetched.close(clear=true)
      release_input_failure(failure)
      stream.cancel_input()
    }
  }
  context.inputs.push({
    id,
    next,
    try_next,
    cancel,
    wire_kind,
    codec,
    stream_token: None,
    channel: None,
    sequence: 0UL,
    terminal: false,
    source_ended: false,
    source_failed: false,
    failure,
    pending: None,
  })
  StreamReferenceValue(Some(id), None)
}

///|
fn find_input_by_id(
  inputs : Array[InputRegistration],
  id : String,
) -> InputRegistration? {
  for input in inputs {
    if input.id == id {
      return Some(input)
    }
  }
  None
}

///|
fn find_input_by_channel(
  inputs : Array[InputRegistration],
  channel : UInt32,
) -> InputRegistration? {
  for input in inputs {
    if input.channel == Some(channel) {
      return Some(input)
    }
  }
  None
}

///|
pub fn[T] open_output_stream(
  reference : SchemaValue,
  decode : async (SchemaValue) -> T,
  codec : PublicValueCodec,
  wire_kind~ : String = "json",
) -> AgentStream[T] raise {
  let token = match reference {
    StreamReferenceValue(_, Some(token)) => token
    _ =>
      raise BridgeError("Output stream did not contain a durable stream token")
  }
  guard stream_sessions.get(token) is Some(session) else {
    raise BridgeError("Streaming session is detached")
  }
  session.output_codecs[token] = codec
  if session.output_claimed.contains(token) {
    raise BridgeError("stream-conflict: output stream was reused")
  }
  match session.output_lanes.get(token) {
    Some(existing) if existing != wire_kind =>
      raise BridgeError("stream-conflict: output stream lane changed")
    _ => session.output_lanes[token] = wire_kind
  }
  session.output_claimed[token] = true
  stream_sessions.remove(token)
  guard session.output_queues.get(token) is Some(queue) else {
    raise BridgeError("stream-conflict: output stream has no queue")
  }
  remote_stream(
    async fn() {
      match receive_output(session, token, queue) {
        Some(value) => Some(decode(value))
        None => None
      }
    },
    async fn(reason) { cancel_output(session, token, reason) },
  )
}

///|
fn protocol_message(kind : String, fields : Array[(String, Json)]) -> String {
  fields.push(("type", Json::string(kind)))
  fields.push(("version", json_int(1)))
  obj(fields).stringify()
}

///|
async fn read_text(socket : @websocket.Conn) -> Json {
  let message = socket.recv()
  match message.kind {
    @websocket.Text => ()
    @websocket.Binary => raise BridgeError("Expected a v1 text frame")
  }
  let bytes = message.read_all().binary()
  if bytes.length() > MAX_MESSAGE_BYTES {
    raise BridgeError("resource-exhausted: WebSocket message exceeds 32 MiB")
  }
  let decoded = parse_strict_json(@utf8.decode(bytes))
  let object = expect_object(decoded)
  guard number_literal(get_field(object, "version")) == "1" else {
    raise BridgeError("unsupported-version: expected protocol version 1")
  }
  let kind = expect_string(get_field(object, "type"))
  validate_message_fields(object, kind)
  decoded
}

///|
fn validate_acceptance_identity(
  accepted : Map[String, Json],
  attempt_id : String,
  previous : (AgentId, String)?,
) -> (AgentId, String) raise {
  if expect_string(get_field(accepted, "attemptId")) != attempt_id {
    raise BridgeError("attempt-conflict: acceptance attempt does not match")
  }
  let owner = get_field(accepted, "agentId")
  validate_object_fields(expect_object(owner), ["componentId", "agentId"], [])
  let agent_id = AgentId::from_json(owner)
  if agent_id.component_id == "" || agent_id.agent_id == "" {
    raise BridgeError("malformed-message: invalid accepted agent identity")
  }
  let key = expect_string(get_field(accepted, "idempotencyKey"))
  ignore(Uuid::parse_canonical(key))
  let identity = (agent_id, key)
  if previous is Some(expected) && expected != identity {
    raise BridgeError(
      "idempotency-conflict: accepted invocation identity changed",
    )
  }
  let token = expect_string(get_field(accepted, "sessionToken"))
  if token == "" || token.length() > 8192 {
    raise BridgeError("token-invalid: invalid session token")
  }
  identity
}

///|
fn validate_message_fields(
  message : Map[String, Json],
  kind : String,
) -> Unit raise {
  let required = match kind {
    "invocationAccepted" =>
      [
        "version", "type", "attemptId", "agentId", "idempotencyKey", "mappings",
        "sessionToken",
      ]
    "invocationRejected" => ["version", "type", "code", "message", "retryable"]
    "invocationResult" => ["version", "type", "mappings", "result"]
    "outputStreamItem" =>
      [
        "version", "type", "channel", "cursorToken", "mappings", "sequence", "value",
      ]
    "stream-end" =>
      ["version", "type", "channel", "direction", "outcome", "sequence"]
    "inputStreamAck" =>
      [
        "version", "type", "channel", "highestContiguousSequence", "mappings", "terminal",
      ]
    "attachmentRevoked" => ["version", "type", "reason"]
    "invocationFinished" => ["version", "type", "outcome"]
    _ => raise BridgeError("malformed-message: unknown protocol variant")
  }
  for field in required {
    if !message.contains(field) {
      raise BridgeError("malformed-message: missing field \{field}")
    }
  }
  message.each((field, _) => {
    let optional = (
        kind == "invocationRejected" &&
        (field == "attemptId" || field == "stderr")
      ) ||
      (kind == "stream-end" && field == "cursorToken")
    if !required.contains(field) && !optional {
      raise BridgeError("malformed-message: unknown field \{field}")
    }
  })
}

///|
/// The error for the server-reported failure `report`, a rejection or the
/// failure outcome of `invocationFinished`: a failure of the agent carries its
/// code, message and stderr; any other failure is its message.
fn reported_failure(report : Map[String, Json]) -> BridgeError raise {
  let code = expect_string(get_field(report, "code"))
  let message = expect_string(get_field(report, "message"))
  let stderr = match field_opt(report, "stderr") {
    Some(value) => Some(expect_string(value))
    None => None
  }
  match code {
    "invocation-failed" | "previous-invocation-failed" =>
      InvocationFailed(code~, message~, stderr~)
    _ => BridgeError(message)
  }
}

///|
/// Validates the failure outcome of `invocationFinished` and returns its error.
fn failure_outcome(outcome : Map[String, Json]) -> BridgeError raise {
  validate_object_fields(outcome, ["kind", "code", "message"], ["stderr"])
  reported_failure(outcome)
}

///|
fn parse_canonical_u64(value : Json) -> UInt64 raise {
  let text = expect_string(value)
  if text == "" || (text.length() > 1 && text[0] == '0') {
    raise BridgeError("malformed-message: non-canonical unsigned decimal")
  }
  @string.parse_uint64(text) catch {
    _ => raise BridgeError("invalid-sequence: unsigned decimal overflow")
  }
}

///|
fn encode_binary_envelope(metadata : Json, payload : Bytes) -> Bytes raise {
  let metadata_bytes = @utf8.encode(metadata.stringify())
  if metadata_bytes.length() > MAX_BINARY_METADATA_BYTES {
    raise BridgeError("resource-exhausted: binary metadata exceeds 16 KiB")
  }
  if metadata_bytes.length() + payload.length() + 4 > MAX_MESSAGE_BYTES {
    raise BridgeError("resource-exhausted: binary message exceeds 32 MiB")
  }
  Bytes::makei(4 + metadata_bytes.length() + payload.length(), i => {
    if i < 4 {
      ((metadata_bytes.length() >> ((3 - i) * 8)) & 0xff).to_byte()
    } else if i < 4 + metadata_bytes.length() {
      metadata_bytes[i - 4]
    } else {
      payload[i - 4 - metadata_bytes.length()]
    }
  })
}

///|
fn decode_binary_envelope(frame : Bytes) -> (Json, Bytes) raise {
  if frame.length() < 4 {
    raise BridgeError("malformed-message: truncated binary length prefix")
  }
  if frame.length() > MAX_MESSAGE_BYTES {
    raise BridgeError("resource-exhausted: binary message exceeds 32 MiB")
  }
  let metadata_length = (frame[0].to_int() << 24) |
    (frame[1].to_int() << 16) |
    (frame[2].to_int() << 8) |
    frame[3].to_int()
  if metadata_length > MAX_BINARY_METADATA_BYTES {
    raise BridgeError("resource-exhausted: binary metadata exceeds 16 KiB")
  }
  if metadata_length > frame.length() - 4 {
    raise BridgeError("malformed-message: metadata length exceeds message")
  }
  let metadata = parse_strict_json(@utf8.decode(frame[4:4 + metadata_length]))
  (metadata, frame[4 + metadata_length:].to_owned())
}

///|
fn validate_object_fields(
  value : Map[String, Json],
  required : Array[String],
  optional : Array[String],
) -> Unit raise {
  for field in required {
    if !value.contains(field) {
      raise BridgeError("malformed-message: missing field \{field}")
    }
  }
  value.each((field, _) => {
    if !required.contains(field) && !optional.contains(field) {
      raise BridgeError("malformed-message: unknown field \{field}")
    }
  })
}

///|
fn channel_is_installed(session : LiveStreamSession, channel : UInt32) -> Bool {
  let mut found = false
  session.input_channels.each((_, installed) => {
    if installed == channel {
      found = true
    }
  })
  session.output_channels.each((_, installed) => {
    if installed == channel {
      found = true
    }
  })
  found
}

///|
fn stable_mapping_count(session : LiveStreamSession) -> Int32 {
  let mut count = 0
  session.input_tokens.each((_, _) => count += 1)
  session.output_queues.each((_, _) => count += 1)
  count
}

///|
fn trim_pending_u8(pending : PendingInput, high_water : UInt64) -> Int32 raise {
  guard pending.payload is PendingU8(bytes) else {
    raise BridgeError("invalid-sequence: partial ACK for non-packed input")
  }
  if high_water <= pending.start || high_water >= pending.start + pending.count {
    raise BridgeError(
      "invalid-sequence: partial packed ACK is outside pending data",
    )
  }
  let dropped = (high_water - pending.start).to_int()
  pending.start = high_water
  pending.count -= dropped.to_uint64()
  pending.payload = PendingU8(bytes[dropped:].to_owned())
  pending.bytes -= dropped
  dropped
}

///|
fn release_input_replay(input : InputRegistration) -> Unit {
  match input.pending {
    Some(pending) => {
      input.failure.replay_bytes.val -= pending.bytes
      input.pending = None
    }
    None => ()
  }
  release_input_failure(input.failure)
}

///|
fn release_input_failure(failure : InputFailure) -> Unit {
  failure.disposed = true
  failure.replay_bytes.val -= failure.bytes
  failure.diagnostic = ""
  failure.bytes = 0
}

///|
fn apply_input_high_water(
  session : LiveStreamSession,
  input : InputRegistration,
  high_water : UInt64,
  terminal : Bool,
) -> Unit raise {
  match input.pending {
    Some(pending) => {
      validate_ack_window(
        input.sequence,
        pending.start,
        pending.count,
        high_water,
      )
      match pending.payload {
        PendingEnd(_) => {
          if high_water != pending.start {
            raise BridgeError(
              "invalid-sequence: terminal ACK advanced sequence",
            )
          }
          if terminal {
            release_input_replay(input)
            input.terminal = true
          }
        }
        _ => {
          if terminal {
            raise BridgeError(
              "invalid-sequence: ACK reports an unsent terminal",
            )
          }
          let acknowledged = high_water - pending.start
          if acknowledged == pending.count {
            session.input_replay_bytes.val -= pending.bytes
            input.pending = None
          } else if acknowledged > 0UL {
            let dropped = trim_pending_u8(pending, high_water)
            session.input_replay_bytes.val -= dropped
          }
        }
      }
    }
    None => {
      validate_ack_window(input.sequence, input.sequence, 0UL, high_water)
      if terminal != input.terminal {
        raise BridgeError(
          "invalid-sequence: ACK terminal conflicts with local state",
        )
      }
    }
  }
  input.sequence = high_water
}

///|
fn install_mappings(
  session : LiveStreamSession,
  message : Map[String, Json],
) -> Unit raise {
  let mappings = expect_array(get_field(message, "mappings"))
  if mappings.length() > 4096 {
    raise BridgeError("resource-exhausted: too many stream mappings")
  }
  let channels : Map[UInt32, Bool] = Map([])
  let tokens : Map[String, Bool] = Map([])
  let provisionals : Map[String, Bool] = Map([])
  for mapping_json in mappings {
    let mapping = expect_object(mapping_json)
    validate_object_fields(mapping, ["channel", "direction", "streamToken"], [
      "inputHighWater", "provisionalRef",
    ])
    let channel = @string.parse_uint(
      number_literal(get_field(mapping, "channel")),
    )
    if channel == 0U {
      raise BridgeError("invalid-channel: channel is outside the u32 range")
    }
    if channels.contains(channel) || channel_is_installed(session, channel) {
      raise BridgeError("invalid-channel: duplicate channel mapping")
    }
    channels[channel] = true
    let token = expect_string(get_field(mapping, "streamToken"))
    if token == "" || token.length() > 8192 || tokens.contains(token) {
      raise BridgeError("stream-conflict: invalid or duplicate stream token")
    }
    tokens[token] = true
    let known_token = session.input_tokens.contains(token) ||
      session.output_queues.contains(token)
    if !known_token && stable_mapping_count(session) >= 4096 {
      raise BridgeError("resource-exhausted: too many stream mappings")
    }
    match expect_string(get_field(mapping, "direction")) {
      "input" => {
        let high_water = expect_object(get_field(mapping, "inputHighWater"))
        validate_object_fields(high_water, ["sequence", "terminal"], [])
        let input = match field_opt(mapping, "provisionalRef") {
          Some(value) => {
            let provisional = expect_string(value)
            if provisional == "" || provisionals.contains(provisional) {
              raise BridgeError("stream-conflict: duplicate provisional stream")
            }
            provisionals[provisional] = true
            guard find_input_by_id(session.inputs, provisional) is Some(input) else {
              raise BridgeError("stream-conflict: unknown provisional stream")
            }
            input
          }
          None => {
            guard session.input_tokens.get(token) is Some(input) else {
              raise BridgeError("stream-conflict: unknown resumed input stream")
            }
            input
          }
        }
        if session.output_queues.contains(token) {
          raise BridgeError("stream-conflict: output token rebound as input")
        }
        if input.stream_token is Some(existing) && existing != token {
          raise BridgeError("stream-conflict: input stream token changed")
        }
        session.input_channels[input.id] = channel
        session.input_tokens[token] = input
        input.stream_token = Some(token)
        input.channel = Some(channel)
        apply_input_high_water(
          session,
          input,
          parse_canonical_u64(get_field(high_water, "sequence")),
          expect_bool(get_field(high_water, "terminal")),
        )
      }
      "output" => {
        if field_opt(mapping, "provisionalRef") is Some(_) ||
          field_opt(mapping, "inputHighWater") is Some(_) {
          raise BridgeError(
            "malformed-message: output mapping has input fields",
          )
        }
        if session.input_tokens.contains(token) {
          raise BridgeError("stream-conflict: input token rebound as output")
        }
        if !session.output_queues.contains(token) {
          session.output_queues[token] = @aqueue.Queue(
            kind=@aqueue.Blocking(MAX_QUEUED_OUTPUT_ITEMS),
          )
          session.output_sequences[token] = 0UL
          session.cursor_sequences[token] = 0UL
          session.output_terminals[token] = false
          session.cursor_terminals[token] = false
        }
        session.output_channels[token] = channel
        if !session.output_claimed.contains(token) {
          stream_sessions[token] = session
        }
      }
      _ => raise BridgeError("malformed-message: invalid mapping direction")
    }
  }
}

///|
async fn send_pending_input(
  session : LiveStreamSession,
  input : InputRegistration,
  pending : PendingInput,
) -> Unit {
  guard input.channel is Some(channel) else {
    raise BridgeError("invalid-channel: input has no attachment mapping")
  }
  match pending.payload {
    PendingJson(value) =>
      send_session_text(
        session,
        protocol_message("inputStreamItem", [
          ("channel", json_uint(channel)),
          ("sequence", Json::string(pending.start.to_string())),
          ("value", input.codec.encode(value)),
        ]),
      )
    PendingU8(payload) => {
      let metadata = obj([
        ("channel", json_uint(channel)),
        ("itemCount", Json::string(pending.count.to_string())),
        ("kind", Json::string("input-u8")),
        ("sequence", Json::string(pending.start.to_string())),
        ("version", json_int(1)),
      ])
      send_session_binary(session, encode_binary_envelope(metadata, payload))
    }
    PendingBinary(payload, mime) => {
      let fields = [
        ("channel", json_uint(channel)),
        ("itemCount", Json::string("1")),
        ("kind", Json::string("input-binary")),
        ("sequence", Json::string(pending.start.to_string())),
        ("version", json_int(1)),
      ]
      match mime {
        Some(value) => fields.push(("mimeType", Json::string(value)))
        None => ()
      }
      send_session_binary(session, encode_binary_envelope(obj(fields), payload))
    }
    PendingEnd(outcome) =>
      send_session_text(
        session,
        protocol_message("stream-end", [
          ("channel", json_uint(channel)),
          ("direction", Json::string("input")),
          ("outcome", outcome),
          ("sequence", Json::string(pending.start.to_string())),
        ]),
      )
  }
}

///|
fn input_failure_outcome(diagnostic : String) -> Json {
  obj([
    (
      "error",
      obj([
        ("kind", Json::string("interrupted")),
        ("message", Json::string(diagnostic)),
      ]),
    ),
    ("kind", Json::string("failed")),
  ])
}

///|
fn admit_input_failure(failure : InputFailure, error : Error) -> Bool {
  if failure.disposed {
    return false
  }
  let diagnostic = @debug.to_string(error)
  let bytes = @utf8.encode(input_failure_outcome(diagnostic).stringify()).length()
  if bytes > MAX_INPUT_REPLAY_BYTES ||
    failure.replay_bytes.val + bytes > MAX_INPUT_REPLAY_BYTES {
    return false
  }
  failure.replay_bytes.val += bytes
  failure.diagnostic = diagnostic
  failure.bytes = bytes
  true
}

///|
fn retain_input_failure(input : InputRegistration, error : Error) -> Unit raise {
  if input.failure.rejected ||
    (input.failure.bytes == 0 && !admit_input_failure(input.failure, error)) {
    raise BridgeError("resource-exhausted: input replay budget exceeded")
  }
  input.source_failed = true
}

///|
async fn cancel_failed_input(
  session : LiveStreamSession,
  input : InputRegistration,
) -> Unit {
  let pending = {
    start: input.sequence,
    count: 0UL,
    payload: PendingEnd(input_failure_outcome(input.failure.diagnostic)),
    bytes: input.failure.bytes,
  }
  input.failure.diagnostic = ""
  input.failure.bytes = 0
  input.pending = Some(pending)
  send_pending_input(session, input, pending)
}

///|
async fn prepare_input(
  session : LiveStreamSession,
  input : InputRegistration,
) -> Unit {
  if input.source_failed {
    cancel_failed_input(session, input)
    return
  }
  if input.source_ended {
    let pending = {
      start: input.sequence,
      count: 0UL,
      payload: PendingEnd(obj([("kind", Json::string("finished"))])),
      bytes: 0,
    }
    input.pending = Some(pending)
    send_pending_input(session, input, pending)
    return
  }
  let mut pending : PendingInput? = None
  if input.wire_kind == "u8" {
    let next = try (input.next)() catch {
      error => Err(error)
    } noraise {
      value => Ok(value)
    }
    let bytes : Array[Byte] = []
    match next {
      Err(error) => retain_input_failure(input, error)
      Ok(None) => input.source_ended = true
      Ok(Some(U8Value(byte))) => bytes.push(byte.to_byte())
      Ok(Some(_)) =>
        raise BridgeError("stream-conflict: input did not match the u8 lane")
    }
    while !input.source_ended &&
          !input.source_failed &&
          bytes.length() < MAX_PACKED_U8_BYTES {
      let prefetched = try (input.try_next)() catch {
        error => {
          retain_input_failure(input, error)
          None
        }
      } noraise {
        value => value
      }
      match prefetched {
        Some(Some(U8Value(byte))) => bytes.push(byte.to_byte())
        Some(Some(_)) =>
          raise BridgeError("stream-conflict: input did not match the u8 lane")
        Some(None) => input.source_ended = true
        None => break
      }
    }
    if !bytes.is_empty() {
      let payload = Bytes::makei(bytes.length(), index => bytes[index])
      pending = Some({
        start: input.sequence,
        count: payload.length().to_uint64(),
        payload: PendingU8(payload),
        bytes: payload.length(),
      })
    }
  } else {
    let next = try (input.next)() catch {
      error => Err(error)
    } noraise {
      value => Ok(value)
    }
    match next {
      Err(error) => retain_input_failure(input, error)
      Ok(None) => input.source_ended = true
      Ok(Some(value)) =>
        match (input.wire_kind, value) {
          ("json", value) => {
            let bytes = input.codec.encode(value).stringify().length()
            pending = Some({
              start: input.sequence,
              count: 1UL,
              payload: PendingJson(value),
              bytes,
            })
          }
          ("binary", BinaryValue(payload, mime)) => {
            ignore(input.codec.encode(BinaryValue(payload, mime)))
            if payload.length() > MAX_LOGICAL_VALUE_BYTES {
              raise BridgeError(
                "resource-exhausted: binary input exceeds 16 MiB",
              )
            }
            pending = Some({
              start: input.sequence,
              count: 1UL,
              payload: PendingBinary(payload, mime),
              bytes: payload.length(),
            })
          }
          _ =>
            raise BridgeError(
              "stream-conflict: input did not match its direct lane",
            )
        }
    }
  }
  match pending {
    Some(pending) => {
      if pending.bytes > MAX_INPUT_REPLAY_BYTES {
        raise BridgeError(
          "resource-exhausted: unacknowledged input exceeds 16 MiB",
        )
      }
      if session.input_replay_bytes.val + pending.bytes > MAX_INPUT_REPLAY_BYTES {
        raise BridgeError("resource-exhausted: input replay budget exceeded")
      }
      session.input_replay_bytes.val += pending.bytes
      input.pending = Some(pending)
      send_pending_input(session, input, pending)
    }
    None if input.source_failed => cancel_failed_input(session, input)
    None => prepare_input(session, input)
  }
}

///|
async fn pump_next_input(session : LiveStreamSession) -> Unit {
  for input in session.inputs {
    if input.pending is Some(_) {
      return
    }
  }
  let count = session.inputs.length()
  for offset = 0; offset < count; offset = offset + 1 {
    let index = (session.next_input_index + offset) % count
    let input = session.inputs[index]
    if !input.terminal && input.channel is Some(_) {
      session.next_input_index = (index + 1) % count
      prepare_input(session, input)
      return
    }
  }
}

///|
async fn acknowledge_input(
  session : LiveStreamSession,
  message : Map[String, Json],
) -> Unit {
  let channel = @string.parse_uint(
    number_literal(get_field(message, "channel")),
  )
  guard find_input_by_channel(session.inputs, channel) is Some(input) else {
    raise BridgeError("invalid-channel: ACK references unknown input")
  }
  apply_input_high_water(
    session,
    input,
    parse_canonical_u64(get_field(message, "highestContiguousSequence")),
    expect_bool(get_field(message, "terminal")),
  )
  install_mappings(session, message)
  pump_next_input(session)
}

///|
fn validate_ack_window(
  sequence : UInt64,
  pending_start : UInt64,
  pending_count : UInt64,
  high_water : UInt64,
) -> Unit raise {
  if high_water < sequence {
    raise BridgeError("invalid-sequence: ACK moved backwards")
  }
  if high_water > pending_start + pending_count {
    raise BridgeError("invalid-sequence: ACK exceeds sent input")
  }
}

///|
async fn queue_output(
  session : LiveStreamSession,
  message : Map[String, Json],
) -> Unit {
  let channel = @string.parse_uint(
    number_literal(get_field(message, "channel")),
  )
  let mut token : String? = None
  session.output_channels.each((candidate, mapped) => {
    if mapped == channel {
      token = Some(candidate)
    }
  })
  guard token is Some(token) else {
    raise BridgeError("invalid-channel: output item references unknown channel")
  }
  let sequence = parse_canonical_u64(get_field(message, "sequence"))
  let expected = session.output_sequences.get(token).unwrap_or(0UL)
  if session.output_terminals.get(token).unwrap_or(false) {
    raise BridgeError("protocol-error: output item followed its terminal")
  }
  if sequence != expected || sequence == U64_MAX {
    raise BridgeError("invalid-sequence: output item is not contiguous")
  }
  match session.output_lanes.get(token) {
    Some(existing) if existing != "json" =>
      raise BridgeError("stream-conflict: output stream lane changed")
    _ => session.output_lanes[token] = "json"
  }
  let cursor = expect_string(get_field(message, "cursorToken"))
  if cursor == "" || cursor.length() > 8192 {
    raise BridgeError("malformed-message: invalid output cursor")
  }
  guard session.output_codecs.get(token) is Some(codec) else {
    raise BridgeError("stream-conflict: output stream has no element codec")
  }
  let value = codec.decode(get_field(message, "value"))
  codec.stream_elements.each((nested_token, nested_codec) => {
    session.output_codecs[nested_token] = nested_codec
  })
  let bytes = codec.encode(value).stringify().length()
  if bytes > MAX_LOGICAL_VALUE_BYTES {
    raise BridgeError("resource-exhausted: logical output exceeds 16 MiB")
  }
  session.output_sequences[token] = expected + 1UL
  if session.output_cancelled.contains(token) {
    return
  }
  enqueue_output(session, token, {
    values: [value],
    index: 0,
    cursor: Some(cursor),
    error: None,
    bytes,
    next_sequence: expected + 1UL,
    terminal: false,
  })
}

///|
async fn enqueue_output(
  session : LiveStreamSession,
  token : String,
  batch : OutputBatch,
) -> Unit {
  let bytes = batch.bytes
  if bytes > MAX_QUEUED_OUTPUT_BYTES {
    raise BridgeError("resource-exhausted: one output exceeds queue byte limit")
  }
  for ;; {
    if session.finished || session.output_cancelled.contains(token) {
      return
    }
    let stream_bytes = session.queue_bytes.get(token).unwrap_or(0) + bytes
    // Queue ownership and its byte charge change without a suspension point.
    if stream_bytes <= MAX_QUEUED_OUTPUT_BYTES &&
      session.total_queue_bytes + bytes <= MAX_QUEUED_OUTPUT_BYTES * 2 &&
      session.output_queues[token].try_put(batch) {
      session.queue_bytes[token] = stream_bytes
      session.total_queue_bytes += bytes
      return
    }
    session.output_space.wait()
  }
}

///|
fn release_output(
  session : LiveStreamSession,
  token : String,
  bytes : Int32,
) -> Unit {
  session.queue_bytes[token] = session.queue_bytes.get(token).unwrap_or(0) -
    bytes
  session.total_queue_bytes -= bytes
  session.output_space.broadcast()
}

///|
async fn queue_binary_output(
  session : LiveStreamSession,
  frame : Bytes,
) -> Unit {
  let (metadata_json, payload) = decode_binary_envelope(frame)
  let metadata = expect_object(metadata_json)
  guard number_literal(get_field(metadata, "version")) == "1" else {
    raise BridgeError("unsupported-version: binary metadata version")
  }
  let kind = expect_string(get_field(metadata, "kind"))
  let optional = if kind == "output-binary" { ["mimeType"] } else { [] }
  validate_object_fields(
    metadata,
    ["version", "kind", "channel", "sequence", "itemCount", "cursorToken"],
    optional,
  )
  let channel = @string.parse_uint(
    number_literal(get_field(metadata, "channel")),
  )
  let mut token : String? = None
  session.output_channels.each((candidate, mapped) => {
    if mapped == channel {
      token = Some(candidate)
    }
  })
  guard token is Some(token) else {
    raise BridgeError("invalid-channel: binary output")
  }
  let sequence = parse_canonical_u64(get_field(metadata, "sequence"))
  let item_count = parse_canonical_u64(get_field(metadata, "itemCount"))
  let expected = session.output_sequences.get(token).unwrap_or(0UL)
  if session.output_terminals.get(token).unwrap_or(false) {
    raise BridgeError("protocol-error: binary output followed its terminal")
  }
  if sequence != expected {
    raise BridgeError("invalid-sequence: binary output is not contiguous")
  }
  let cursor = expect_string(get_field(metadata, "cursorToken"))
  if cursor == "" || cursor.length() > 8192 {
    raise BridgeError("malformed-message: empty output cursor")
  }
  let values : Array[SchemaValue] = []
  match kind {
    "output-u8" => {
      match session.output_lanes.get(token) {
        Some(existing) if existing != "u8" =>
          raise BridgeError("stream-conflict: output stream lane changed")
        _ => session.output_lanes[token] = "u8"
      }
      if payload.length() == 0 ||
        payload.length() > MAX_PACKED_U8_BYTES ||
        item_count != payload.length().to_uint64() ||
        field_opt(metadata, "mimeType") is Some(_) {
        raise BridgeError("malformed-message: invalid packed u8 envelope")
      }
      if U64_MAX - sequence < item_count {
        raise BridgeError("invalid-sequence: packed u8 overflow")
      }
      for byte in payload {
        values.push(U8Value(byte.to_int()))
      }
    }
    "output-binary" => {
      match session.output_lanes.get(token) {
        Some(existing) if existing != "binary" =>
          raise BridgeError("stream-conflict: output stream lane changed")
        _ => session.output_lanes[token] = "binary"
      }
      if item_count != 1UL || payload.length() > MAX_LOGICAL_VALUE_BYTES {
        raise BridgeError("malformed-message: invalid binary item envelope")
      }
      let mime = match field_opt(metadata, "mimeType") {
        Some(value) => Some(validate_mime_type(expect_string(value)))
        None => None
      }
      values.push(BinaryValue(payload, mime))
    }
    _ => raise BridgeError("malformed-message: invalid server binary kind")
  }
  guard session.output_codecs.get(token) is Some(codec) else {
    raise BridgeError("stream-conflict: output stream has no element codec")
  }
  for value in values {
    ignore(codec.encode(value))
  }
  session.output_sequences[token] = sequence + item_count
  if session.output_cancelled.contains(token) {
    return
  }
  enqueue_output(session, token, {
    values,
    index: 0,
    cursor: Some(cursor),
    error: None,
    bytes: payload.length(),
    next_sequence: sequence + item_count,
    terminal: false,
  })
}

///|
fn validate_mime_type(value : String) -> String raise {
  guard value.find("/") is Some(slash) &&
    slash > 0 &&
    slash + 1 < value.length() else {
    raise BridgeError("malformed-message: invalid MIME type")
  }
  for char in value {
    let ascii_alphanumeric = (char >= 'A' && char <= 'Z') ||
      (char >= 'a' && char <= 'z') ||
      (char >= '0' && char <= '9')
    if !(ascii_alphanumeric || "!#$&^_.+-/".contains(char.to_string())) {
      raise BridgeError("malformed-message: invalid MIME type")
    }
  }
  value
}

///|
async fn receive_output(
  session : LiveStreamSession,
  token : String,
  queue : @aqueue.Queue[OutputBatch],
) -> SchemaValue? {
  for ;; {
    let batch = match session.output_active.get(token) {
      Some(active) => active
      None => {
        let batch = queue.get()
        session.output_space.broadcast()
        batch
      }
    }
    if batch.index < batch.values.length() {
      let value = batch.values[batch.index]
      batch.index += 1
      if batch.index == batch.values.length() {
        session.output_active.remove(token)
        release_output(session, token, batch.bytes)
        match batch.cursor {
          Some(cursor) => {
            session.cursors[token] = cursor
            session.cursor_sequences[token] = batch.next_sequence
            session.cursor_terminals[token] = batch.terminal
          }
          None => ()
        }
      } else {
        session.output_active[token] = batch
      }
      return Some(value)
    }
    session.output_active.remove(token)
    release_output(session, token, batch.bytes)
    match batch.cursor {
      Some(cursor) => {
        session.cursors[token] = cursor
        session.cursor_sequences[token] = batch.next_sequence
        session.cursor_terminals[token] = batch.terminal
      }
      None => ()
    }
    match batch.error {
      Some(error) => raise error
      None => return None
    }
  }
}

///|
fn output_token_for_channel(
  session : LiveStreamSession,
  channel : UInt32,
) -> String raise {
  let mut token : String? = None
  session.output_channels.each((candidate, mapped) => {
    if mapped == channel {
      token = Some(candidate)
    }
  })
  guard token is Some(token) else {
    raise BridgeError("invalid-channel: unknown output channel")
  }
  token
}

///|
fn decode_stream_outcome(value : Json) -> BridgeError? raise {
  let outcome = expect_object(value)
  match expect_string(get_field(outcome, "kind")) {
    "finished" => {
      validate_object_fields(outcome, ["kind"], [])
      None
    }
    "consumer-cancelled" => {
      validate_object_fields(outcome, ["kind"], ["details"])
      let details = match field_opt(outcome, "details") {
        None | Some(Null) => None
        Some(value) => Some(expect_string(value))
      }
      Some(ConsumerCancelled(details))
    }
    "failed" => {
      validate_object_fields(outcome, ["kind", "error"], [])
      let error = expect_object(get_field(outcome, "error"))
      let failure = match expect_string(get_field(error, "kind")) {
        "failed" => {
          validate_object_fields(error, ["kind", "value"], [])
          let typed = expect_object(get_field(error, "value"))
          validate_object_fields(typed, ["graph", "value"], [])
          let graph = get_field(typed, "graph")
          let value = get_field(typed, "value")
          let codec = public_value_codec(graph.stringify())
          ignore(codec.decode(value))
          if !codec.stream_elements.is_empty() {
            raise BridgeError(
              "malformed-message: stream failure contains a stream reference",
            )
          }
          StreamFailure::Failed(graph~, value~)
        }
        "trapped" | "interrupted" | "resource-exhausted" as kind => {
          validate_object_fields(error, ["kind", "message"], [])
          let message = expect_string(get_field(error, "message"))
          match kind {
            "trapped" => Trapped(message)
            "interrupted" => Interrupted(message)
            _ => ResourceExhausted(message)
          }
        }
        "cancelled" => {
          validate_object_fields(error, ["kind"], [])
          Cancelled
        }
        "abandoned" => {
          validate_object_fields(error, ["kind"], [])
          Abandoned
        }
        _ => raise BridgeError("malformed-message: invalid stream error")
      }
      Some(StreamFailed(failure))
    }
    _ => raise BridgeError("malformed-message: invalid stream outcome")
  }
}

///|
fn validate_server_stream_end(message : Map[String, Json]) -> String raise {
  validate_message_fields(message, "stream-end")
  let direction = expect_string(get_field(message, "direction"))
  ignore(decode_stream_outcome(get_field(message, "outcome")))
  if direction != "output" && direction != "input" {
    raise BridgeError(
      "protocol-error: invalid server stream terminal direction",
    )
  }
  let channel = @string.parse_uint(
    number_literal(get_field(message, "channel")),
  )
  if channel == 0U {
    raise BridgeError("invalid-channel: channel zero is reserved")
  }
  ignore(parse_canonical_u64(get_field(message, "sequence")))
  match field_opt(message, "cursorToken") {
    None | Some(Null) => ()
    Some(value) => {
      let cursor = expect_string(value)
      if cursor == "" || cursor.length() > 8192 {
        raise BridgeError("malformed-message: invalid terminal cursor")
      }
    }
  }
  direction
}

///|
async fn queue_output_end(
  session : LiveStreamSession,
  message : Map[String, Json],
) -> Unit {
  let channel = @string.parse_uint(
    number_literal(get_field(message, "channel")),
  )
  let token = output_token_for_channel(session, channel)
  let sequence = parse_canonical_u64(get_field(message, "sequence"))
  if session.output_terminals.get(token).unwrap_or(false) {
    raise BridgeError("protocol-error: duplicate output terminal")
  }
  if sequence != session.output_sequences.get(token).unwrap_or(0UL) {
    raise BridgeError("invalid-sequence: output terminal is not contiguous")
  }
  let error = decode_stream_outcome(get_field(message, "outcome"))
  let cursor = match field_opt(message, "cursorToken") {
    None | Some(Null) => None
    Some(value) => {
      let cursor = expect_string(value)
      if cursor == "" || cursor.length() > 8192 {
        raise BridgeError("malformed-message: invalid terminal cursor")
      }
      Some(cursor)
    }
  }
  session.output_terminals[token] = true
  if session.output_cancelled.contains(token) {
    return
  }
  let bytes = @utf8.encode(Json(message).stringify()).length()
  enqueue_output(session, token, {
    values: [],
    index: 0,
    cursor,
    error,
    bytes,
    next_sequence: sequence,
    terminal: true,
  })
}

///|
fn drain_output_queue(
  session : LiveStreamSession,
  token : String,
  queue : @aqueue.Queue[OutputBatch],
) -> Unit {
  match session.output_active.get(token) {
    Some(active) => {
      session.output_active.remove(token)
      release_output(session, token, active.bytes)
    }
    None => ()
  }
  for ;; {
    let next = try queue.try_get() catch {
      error => Err(error)
    } noraise {
      value => Ok(value)
    }
    match next {
      Ok(Some(batch)) => release_output(session, token, batch.bytes)
      Ok(None) | Err(_) => return
    }
  }
}

///|
async fn fail_session(session : LiveStreamSession, error : Error) -> Unit {
  session.finished = true
  session.output_space.broadcast()
  session.result_queue.close(error~, clear=true)
  session.output_queues.each((token, queue) => {
    drain_output_queue(session, token, queue)
    queue.close(error~, clear=true)
    stream_sessions.remove(token)
  })
  for input in session.inputs {
    if !input.terminal {
      input.terminal = true
      release_input_replay(input)
      (input.cancel)()
    }
  }
  session.socket.close()
}

///|
fn release_unclaimed_outputs(session : LiveStreamSession) -> Unit {
  session.output_queues.each((token, queue) => {
    stream_sessions.remove(token)
    if !session.output_claimed.contains(token) {
      drain_output_queue(session, token, queue)
      queue.close(clear=true)
      session.output_codecs.remove(token)
    }
  })
}

///|
async fn dispatch_server_message(
  session : LiveStreamSession,
  message : Map[String, Json],
) -> Bool {
  match expect_string(get_field(message, "type")) {
    "invocationResult" => {
      if session.result_seen {
        raise BridgeError("protocol-error: duplicate invocation result")
      }
      install_mappings(session, message)
      let result = expect_object(get_field(message, "result"))
      let kind = expect_string(get_field(result, "kind"))
      let value = match kind {
        "value" => {
          validate_object_fields(result, ["graph", "kind", "value"], [])
          let graph = get_field(result, "graph")
          ignore(expect_object(graph))
          let wire_codec = public_value_codec(graph.stringify())
          let value = wire_codec.decode(get_field(result, "value"))
          ignore(session.result_codec.encode(value))
          session.result_codec.stream_elements.each((token, codec) => {
            session.output_codecs[token] = codec
          })
          Some(value)
        }
        "none" => {
          validate_object_fields(result, ["kind"], [])
          None
        }
        _ => raise BridgeError("malformed-message: invalid invocation result")
      }
      session.result_seen = true
      session.result_queue.put({
        agent_id: session.agent_id,
        idempotency_key: session.idempotency_key,
        result: value,
        component_revision: None,
      })
    }
    "outputStreamItem" => {
      install_mappings(session, message)
      queue_output(session, message)
    }
    "inputStreamAck" => acknowledge_input(session, message)
    "stream-end" => {
      let direction = validate_server_stream_end(message)
      if direction == "output" {
        queue_output_end(session, message)
        return false
      }
      let channel = @string.parse_uint(
        number_literal(get_field(message, "channel")),
      )
      guard find_input_by_channel(session.inputs, channel) is Some(input) else {
        raise BridgeError(
          "invalid-channel: cancellation references unknown input",
        )
      }
      let sequence = parse_canonical_u64(get_field(message, "sequence"))
      let sent = match input.pending {
        Some(pending) => pending.start + pending.count
        None => input.sequence
      }
      if sequence < input.sequence || sequence > sent {
        raise BridgeError(
          "invalid-sequence: input cancellation exceeds sent window",
        )
      }
      release_input_replay(input)
      input.terminal = true
      (input.cancel)()
      pump_next_input(session)
    }
    "attachmentRevoked" => {
      if expect_string(get_field(message, "reason")) != "replaced" {
        raise BridgeError("malformed-message: invalid revocation reason")
      }
      resume_session(session)
    }
    "invocationRejected" => {
      match field_opt(message, "attemptId") {
        Some(value) if expect_string(value) != session.attempt_id =>
          raise BridgeError(
            "attempt-conflict: rejection attempt does not match",
          )
        _ => ()
      }
      let code = expect_string(get_field(message, "code"))
      let text = expect_string(get_field(message, "message"))
      if !expect_bool(get_field(message, "retryable")) ||
        code == "invalid-attachment-state" {
        raise reported_failure(message)
      }
      session.socket.close()
      resume_session(session, rejection="\{code}: \{text}")
    }
    "invocationFinished" => {
      let outcome = expect_object(get_field(message, "outcome"))
      match expect_string(get_field(outcome, "kind")) {
        "success" => validate_object_fields(outcome, ["kind"], [])
        "failure" => {
          let error = failure_outcome(outcome)
          if !session.result_seen {
            session.result_queue.close(error~, clear=true)
          }
        }
        _ => raise BridgeError("malformed-message: invalid invocation outcome")
      }
      if expect_string(get_field(outcome, "kind")) == "success" &&
        !session.result_seen {
        raise BridgeError("protocol-error: invocation finished before result")
      }
      session.output_terminals.each((token, terminal) => {
        if !terminal && !session.output_cancelled.contains(token) {
          raise BridgeError(
            "protocol-error: invocation finished before output terminal",
          )
        }
      })
      for input in session.inputs {
        if !input.terminal {
          input.terminal = true
          release_input_replay(input)
          (input.cancel)()
        }
      }
      session.finished = true
      session.output_space.broadcast()
      release_unclaimed_outputs(session)
      session.socket.close()
      return true
    }
    _ =>
      raise BridgeError("protocol-error: message is illegal after acceptance")
  }
  false
}

///|
async fn receive_actor(session : LiveStreamSession) -> Unit {
  for ;; {
    let (kind, bytes) = try {
      let raw = session.socket.recv()
      (raw.kind, raw.read_all().binary())
    } catch {
      @websocket.ConnectionClosed(_, _) => {
        if session.finished {
          return
        }
        resume_session(session)
        continue
      }
      @io.ReaderClosed => {
        if session.finished {
          return
        }
        resume_session(session)
        continue
      }
      @os_error.OSError(_) => {
        if session.finished {
          return
        }
        resume_session(session)
        continue
      }
      error => raise error
    }
    match kind {
      @websocket.Binary => queue_binary_output(session, bytes)
      @websocket.Text => {
        if bytes.length() > MAX_MESSAGE_BYTES {
          raise BridgeError(
            "resource-exhausted: WebSocket message exceeds 32 MiB",
          )
        }
        let message = expect_object(parse_strict_json(@utf8.decode(bytes)))
        guard number_literal(get_field(message, "version")) == "1" else {
          raise BridgeError("unsupported-version: expected protocol version 1")
        }
        let kind = expect_string(get_field(message, "type"))
        validate_message_fields(message, kind)
        if dispatch_server_message(session, message) {
          return
        }
      }
    }
  }
}

///|
async fn cancel_output(
  session : LiveStreamSession,
  token : String,
  reason : String,
) -> Unit {
  guard session.output_channels.get(token) is Some(channel) else { return }
  if session.output_cancelled.contains(token) {
    return
  }
  session.output_cancelled[token] = true
  drain_output_queue(session, token, session.output_queues[token])
  session.output_space.broadcast()
  if !session.finished {
    send_session_text(
      session,
      protocol_message("stream-end", [
        ("channel", json_uint(channel)),
        ("direction", Json::string("output")),
        (
          "outcome",
          obj([
            ("details", Json::string(reason)),
            ("kind", Json::string("consumer-cancelled")),
          ]),
        ),
        ("sequence", Json::string("0")),
      ]),
    )
  }
}

///|
pub async fn invoke_streaming_agent(
  context : StreamEncodeContext,
  constructor_codec : PublicValueCodec,
  input_codec : PublicValueCodec,
  output_codec : PublicValueCodec,
  resolved : ResolvedAgent,
  method_name : String,
  method_parameters : SchemaValue,
) -> AgentInvocationResult {
  let server = resolved.configuration.server
  let url = server
    .url()
    .replace(old="http://", new="ws://")
    .replace(old="https://", new="wss://") +
    "/v1/agents/invoke-agent-session"
  let headers : @http.Headers = {
    "Authorization": "Bearer " + server.token(),
    "Sec-WebSocket-Protocol": STREAM_SUBPROTOCOL,
  }
  let inputs = context.inputs
  errdefer (for input in inputs {
    if !input.terminal {
      input.terminal = true
      release_input_replay(input)
      (input.cancel)()
    }
  })
  let idempotency_key = fresh_id()
  let selector : Array[(String, Json)] = [
    ("agentType", Json::string(resolved.agent_type_name)),
    ("application", Json::string(resolved.configuration.app_name)),
    ("constructorParameters", constructor_codec.encode(resolved.parameters)),
    ("environment", Json::string(resolved.configuration.env_name)),
    ("method", Json::string(method_name)),
  ]
  match resolved.phantom_id {
    Some(id) => selector.push(("phantomId", Json::string(id.to_string())))
    None => ()
  }
  let config = Json::array(
    resolved.config.map(entry => {
      obj([
        ("path", Json::array(entry.path.map(Json::string))),
        ("value", entry.value),
      ])
    }),
  )
  let clock = ReconnectClock::new(resolved.configuration.reconnect, url)
  let (socket, accepted, attempt_id) = start_invocation(
    clock,
    url,
    headers,
    inputs,
    input_codec.encode(method_parameters),
    (attempt_id, parameters) => {
      protocol_message("invocationStart", [
        ("attemptId", Json::string(attempt_id)),
        ("config", config),
        ("idempotencyKey", Json::string(idempotency_key)),
        ("methodParameters", parameters),
        ("selector", obj(selector)),
      ])
    },
  )
  let (agent_id, accepted_key) = validate_acceptance_identity(
    accepted,
    attempt_id,
    None,
  )
  let session = {
    socket,
    url,
    headers,
    session_token: None,
    attempt_id,
    agent_id,
    idempotency_key: accepted_key,
    inputs,
    input_channels: {},
    input_tokens: {},
    next_input_index: 0,
    input_replay_bytes: context.replay_bytes,
    output_channels: {},
    output_sequences: {},
    output_queues: {},
    output_active: {},
    output_lanes: {},
    output_codecs: {},
    result_codec: output_codec,
    output_terminals: {},
    output_claimed: {},
    output_cancelled: {},
    queue_bytes: {},
    total_queue_bytes: 0,
    cursors: {},
    cursor_sequences: {},
    cursor_terminals: {},
    result_queue: @aqueue.Queue(kind=@aqueue.Blocking(1)),
    socket_write: @async.Mutex(),
    output_space: @cond.Cond(),
    result_seen: false,
    finished: false,
    actor_owner: None,
    clock,
  }
  session.session_token = Some(
    expect_string(get_field(accepted, "sessionToken")),
  )
  install_mappings(session, accepted)
  output_codec.stream_elements.each((token, codec) => {
    session.output_codecs[token] = codec
  })
  pump_next_input(session)
  session.actor_owner = Some(
    @io.MemoryReader(async fn(_) {
      receive_actor(session) catch {
        error => fail_session(session, error)
      }
    }),
  )
  session.result_queue.get()
}
