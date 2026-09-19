// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
const STREAM_SUBPROTOCOL : String = "golem.agent-invocation.v1"

///|
const MAX_MESSAGE_BYTES : Int = 32 * 1024 * 1024

///|
const MAX_BINARY_METADATA_BYTES : Int = 16 * 1024

///|
const MAX_PACKED_U8_BYTES : Int = 1024 * 1024

///|
const MAX_LOGICAL_VALUE_BYTES : Int = 16 * 1024 * 1024

///|
const MAX_QUEUED_OUTPUT_ITEMS : Int = 256

///|
const MAX_QUEUED_OUTPUT_BYTES : Int = 16 * 1024 * 1024

///|
const MAX_INPUT_REPLAY_BYTES : Int = 16 * 1024 * 1024

///|
const U64_MAX : UInt64 = 18446744073709551615UL

///|
pub(all) enum PendingInputPayload {
  PendingJson(SchemaValue)
  PendingU8(Bytes)
  PendingBinary(Bytes, String?)
  PendingEnd
}

///|
pub(all) struct PendingInput {
  mut start : UInt64
  mut count : UInt64
  mut payload : PendingInputPayload
  mut bytes : Int
}

///|
pub(all) struct InputRegistration {
  id : String
  next : async () -> SchemaValue?
  try_next : () -> SchemaValue?? raise
  cancel : async () -> Unit
  wire_kind : String
  codec : PublicValueCodec
  mut stream_token : String?
  mut channel : UInt?
  mut sequence : UInt64
  mut terminal : Bool
  mut source_ended : Bool
  mut source_failed : Bool
  mut pending : PendingInput?
}

///|
pub struct StreamEncodeContext {
  inputs : Array[InputRegistration]
}

///|
pub fn StreamEncodeContext::new() -> StreamEncodeContext {
  { inputs: [], }
}

///|
priv struct OutputBatch {
  values : Array[SchemaValue]
  mut index : Int
  cursor : String?
  error : String?
  bytes : Int
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
  idempotency_key : String
  inputs : Array[InputRegistration]
  input_channels : Map[String, UInt]
  input_tokens : Map[String, InputRegistration]
  mut next_input_index : Int
  mut input_replay_bytes : Int
  output_channels : Map[String, UInt]
  output_sequences : Map[String, UInt64]
  output_queues : Map[String, @aqueue.Queue[OutputBatch]]
  output_active : Map[String, OutputBatch]
  output_lanes : Map[String, String]
  output_codecs : Map[String, PublicValueCodec]
  result_codec : PublicValueCodec
  output_terminals : Map[String, Bool]
  output_claimed : Map[String, Bool]
  output_cancelled : Map[String, Bool]
  queue_bytes : Map[String, Int]
  mut total_queue_bytes : Int
  cursors : Map[String, String]
  cursor_sequences : Map[String, UInt64]
  cursor_terminals : Map[String, Bool]
  result_queue : @aqueue.Queue[AgentInvocationResult]
  socket_write : @async.Mutex
  output_space : @cond.Cond
  mut result_seen : Bool
  mut finished : Bool
  mut actor_owner : @io.MemoryReader?
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
async fn connect_start_exact_retry(
  url : String,
  headers : @http.Headers,
  frozen_start : String,
) -> (@websocket.Conn, Map[String, Json]) {
  for ;; {
    try {
      let socket = connect_socket(url, headers)
      socket.send_text(frozen_start)
      return (socket, expect_object(read_text(socket)))
    } catch {
      @websocket.HandshakeRejected(message, _) => raise BridgeError(message)
      @websocket.ConnectionClosed(_, _) => @async.sleep(50)
      @io.ReaderClosed => @async.sleep(50)
      @os_error.OSError(_) => @async.sleep(50)
      error => raise error
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
async fn resume_session(session : LiveStreamSession) -> Unit {
  guard session.session_token is Some(token) else {
    raise BridgeError("invalid-attachment-state: accepted session has no token")
  }
  let cursors : Array[Json] = []
  session.cursors.each((_, cursor) => cursors.push(Json::string(cursor)))
  let attempt_id = fresh_id()
  let frozen_resume = protocol_message("resumeAttach", [
    ("attemptId", Json::string(attempt_id)),
    ("operation", Json::string("resume")),
    ("outputCursors", Json::array(cursors)),
    ("sessionToken", Json::string(token)),
  ])
  let mut accepted : Map[String, Json]? = None
  let mut socket : @websocket.Conn? = None
  for ;; {
    try {
      let candidate = connect_socket(session.url, session.headers)
      candidate.send_text(frozen_resume)
      let response = expect_object(read_text(candidate))
      match expect_string(get_field(response, "type")) {
        "invocationAccepted" => {
          accepted = Some(response)
          socket = Some(candidate)
          break
        }
        "invocationRejected" => {
          if expect_bool(get_field(response, "retryable")) {
            break
          }
          raise BridgeError(expect_string(get_field(response, "message")))
        }
        _ =>
          raise BridgeError("invalid-attachment-state: invalid resume response")
      }
    } catch {
      @websocket.ConnectionClosed(_, _) => @async.sleep(50)
      @io.ReaderClosed => @async.sleep(50)
      @os_error.OSError(_) => @async.sleep(50)
      error => raise error
    }
    if accepted is None && socket is None {
      @async.sleep(50)
    }
    if accepted is Some(_) {
      break
    }
  }
  if accepted is None {
    return resume_session(session)
  }
  let accepted = accepted.unwrap()
  let socket = socket.unwrap()
  validate_acceptance_identity(accepted, attempt_id, session.idempotency_key)
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
  random_uuid()
}

///|
pub fn[T] register_input_stream(
  context : StreamEncodeContext,
  stream : AgentStream[T],
  encode : async (T) -> SchemaValue,
  codec : PublicValueCodec,
  wire_kind? : String = "json",
) -> SchemaValue raise {
  stream.acquire()
  let id = fresh_id()
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
            prefetched.close(error~)
            return
          }
        } noraise {
          value => value
        }
        match value {
          Some(value) => {
            let encoded = try encode(value) catch {
              error => {
                prefetched.close(error~)
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
      owner.close()
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
  channel : UInt,
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
  wire_kind? : String = "json",
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
  idempotency_key : String,
) -> Unit raise {
  if expect_string(get_field(accepted, "attemptId")) != attempt_id {
    raise BridgeError("attempt-conflict: acceptance attempt does not match")
  }
  if expect_string(get_field(accepted, "idempotencyKey")) != idempotency_key {
    raise BridgeError("idempotency-conflict: acceptance key does not match")
  }
  let token = expect_string(get_field(accepted, "sessionToken"))
  if token == "" || token.length() > 8192 {
    raise BridgeError("token-invalid: invalid session token")
  }
}

///|
fn validate_message_fields(
  message : Map[String, Json],
  kind : String,
) -> Unit raise {
  let required = match kind {
    "invocationAccepted" =>
      [
        "version", "type", "attemptId", "idempotencyKey", "mappings", "sessionToken",
      ]
    "invocationRejected" => ["version", "type", "code", "message", "retryable"]
    "invocationResult" => ["version", "type", "mappings", "result"]
    "outputStreamItem" =>
      [
        "version", "type", "channel", "cursorToken", "mappings", "sequence", "value",
      ]
    "outputStreamEnd" => ["version", "type", "channel", "outcome", "sequence"]
    "inputStreamAck" =>
      [
        "version", "type", "channel", "highestContiguousSequence", "mappings", "terminal",
      ]
    "streamCancel" => ["version", "type", "channel", "reason"]
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
    let optional = (kind == "invocationRejected" && field == "attemptId") ||
      (kind == "outputStreamEnd" && field == "cursorToken")
    if !required.contains(field) && !optional {
      raise BridgeError("malformed-message: unknown field \{field}")
    }
  })
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
fn channel_is_installed(session : LiveStreamSession, channel : UInt) -> Bool {
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
fn stable_mapping_count(session : LiveStreamSession) -> Int {
  let mut count = 0
  session.input_tokens.each((_, _) => count += 1)
  session.output_queues.each((_, _) => count += 1)
  count
}

///|
fn trim_pending_u8(pending : PendingInput, high_water : UInt64) -> Int raise {
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
        PendingEnd => {
          if high_water != pending.start {
            raise BridgeError(
              "invalid-sequence: terminal ACK advanced sequence",
            )
          }
          if terminal {
            input.pending = None
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
            session.input_replay_bytes -= pending.bytes
            input.pending = None
          } else if acknowledged > 0UL {
            let dropped = trim_pending_u8(pending, high_water)
            session.input_replay_bytes -= dropped
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
  let channels : Map[UInt, Bool] = Map([])
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
    PendingEnd =>
      send_session_text(
        session,
        protocol_message("inputStreamEnd", [
          ("channel", json_uint(channel)),
          ("sequence", Json::string(pending.start.to_string())),
        ]),
      )
  }
}

///|
async fn cancel_failed_input(
  session : LiveStreamSession,
  input : InputRegistration,
) -> Unit {
  guard input.channel is Some(channel) else { return }
  input.terminal = true
  (input.cancel)()
  send_session_text(
    session,
    protocol_message("streamCancel", [
      ("channel", json_uint(channel)),
      ("reason", Json::string("source-unavailable")),
    ]),
  )
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
      payload: PendingEnd,
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
      Err(_) => input.source_failed = true
      Ok(None) => input.source_ended = true
      Ok(Some(U8Value(byte))) => bytes.push(byte.to_byte())
      Ok(Some(_)) =>
        raise BridgeError("stream-conflict: input did not match the u8 lane")
    }
    while !input.source_ended &&
          !input.source_failed &&
          bytes.length() < MAX_PACKED_U8_BYTES {
      let prefetched = try (input.try_next)() catch {
        _ => {
          input.source_failed = true
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
      Err(_) => input.source_failed = true
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
      if session.input_replay_bytes + pending.bytes > MAX_INPUT_REPLAY_BYTES {
        raise BridgeError("resource-exhausted: input replay budget exceeded")
      }
      session.input_replay_bytes += pending.bytes
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
  charge_output(session, token, bytes)
  session.output_queues[token].put({
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
async fn charge_output(
  session : LiveStreamSession,
  token : String,
  bytes : Int,
) -> Unit {
  if bytes > MAX_QUEUED_OUTPUT_BYTES {
    raise BridgeError("resource-exhausted: one output exceeds queue byte limit")
  }
  while session.queue_bytes.get(token).unwrap_or(0) + bytes >
        MAX_QUEUED_OUTPUT_BYTES ||
        session.total_queue_bytes + bytes > MAX_QUEUED_OUTPUT_BYTES * 2 {
    session.output_space.wait()
  }
  let stream_bytes = session.queue_bytes.get(token).unwrap_or(0) + bytes
  session.queue_bytes[token] = stream_bytes
  session.total_queue_bytes += bytes
}

///|
fn release_output(
  session : LiveStreamSession,
  token : String,
  bytes : Int,
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
  charge_output(session, token, payload.length())
  session.output_queues[token].put({
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
      None => queue.get()
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
      Some(error) => raise BridgeError(error)
      None => return None
    }
  }
}

///|
fn output_token_for_channel(
  session : LiveStreamSession,
  channel : UInt,
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
  let outcome = expect_object(get_field(message, "outcome"))
  let kind = expect_string(get_field(outcome, "kind"))
  let error = match kind {
    "ok" => {
      validate_object_fields(outcome, ["kind"], [])
      None
    }
    "error" => {
      validate_object_fields(outcome, ["kind", "code", "message"], [])
      Some(expect_string(get_field(outcome, "message")))
    }
    "cancelled" => {
      validate_object_fields(outcome, ["kind", "reason"], [])
      Some(
        "Output stream cancelled: \{expect_string(get_field(outcome, "reason"))}",
      )
    }
    _ => raise BridgeError("malformed-message: invalid output terminal outcome")
  }
  let cursor = match field_opt(message, "cursorToken") {
    Some(value) => {
      let cursor = expect_string(value)
      if cursor == "" || cursor.length() > 8192 {
        raise BridgeError("malformed-message: invalid terminal cursor")
      }
      Some(cursor)
    }
    None => None
  }
  session.output_terminals[token] = true
  if session.output_cancelled.contains(token) {
    return
  }
  let bytes = message.to_json().stringify().length()
  charge_output(session, token, bytes)
  session.output_queues[token].put({
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
  session.result_queue.close(error~, clear=true)
  session.output_queues.each((token, queue) => {
    drain_output_queue(session, token, queue)
    queue.close(error~, clear=true)
    stream_sessions.remove(token)
  })
  for input in session.inputs {
    if !input.terminal {
      input.terminal = true
      match input.pending {
        Some(pending) => {
          session.input_replay_bytes -= pending.bytes
          input.pending = None
        }
        None => ()
      }
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
  resolved : ResolvedAgent,
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
          validate_object_fields(result, ["kind", "value"], [])
          let value = session.result_codec.decode(get_field(result, "value"))
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
        agent_id: resolved.agent_id.unwrap_or({
          component_id: "",
          agent_id: "",
        }),
        idempotency_key: session.idempotency_key,
        result: value,
        component_revision: None,
      })
    }
    "outputStreamItem" => {
      install_mappings(session, message)
      queue_output(session, message)
    }
    "outputStreamEnd" => queue_output_end(session, message)
    "inputStreamAck" => acknowledge_input(session, message)
    "streamCancel" => {
      let channel = @string.parse_uint(
        number_literal(get_field(message, "channel")),
      )
      guard find_input_by_channel(session.inputs, channel) is Some(input) else {
        raise BridgeError(
          "invalid-channel: cancellation references unknown input",
        )
      }
      match input.pending {
        Some(pending) => {
          session.input_replay_bytes -= pending.bytes
          input.pending = None
        }
        None => ()
      }
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
      raise BridgeError(expect_string(get_field(message, "message")))
    }
    "invocationFinished" => {
      let outcome = expect_object(get_field(message, "outcome"))
      match expect_string(get_field(outcome, "kind")) {
        "success" => validate_object_fields(outcome, ["kind"], [])
        "failure" => {
          validate_object_fields(outcome, ["kind", "code", "message"], [])
          if !session.result_seen {
            session.result_queue.close(
              error=BridgeError(expect_string(get_field(outcome, "message"))),
              clear=true,
            )
          }
        }
        _ => raise BridgeError("malformed-message: invalid invocation outcome")
      }
      if expect_string(get_field(outcome, "kind")) == "success" &&
        !session.result_seen {
        raise BridgeError("protocol-error: invocation finished before result")
      }
      session.output_terminals.each((_, terminal) => {
        if !terminal {
          raise BridgeError(
            "protocol-error: invocation finished before output terminal",
          )
        }
      })
      for input in session.inputs {
        if !input.terminal {
          input.terminal = true
          match input.pending {
            Some(pending) => {
              session.input_replay_bytes -= pending.bytes
              input.pending = None
            }
            None => ()
          }
          (input.cancel)()
        }
      }
      session.finished = true
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
async fn receive_actor(
  session : LiveStreamSession,
  resolved : ResolvedAgent,
) -> Unit {
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
        if dispatch_server_message(session, resolved, message) {
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
  session.output_terminals[token] = true
  drain_output_queue(session, token, session.output_queues[token])
  if !session.finished {
    send_session_text(
      session,
      protocol_message("streamCancel", [
        ("channel", json_uint(channel)),
        ("reason", Json::string(reason)),
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
  let attempt_id = fresh_id()
  let inputs = context.inputs
  let idempotency_key = fresh_id()
  let selector : Array[(String, Json)] = [
    ("agentType", Json::string(resolved.agent_type_name)),
    ("application", Json::string(resolved.configuration.app_name)),
    ("constructorParameters", constructor_codec.encode(resolved.parameters)),
    ("environment", Json::string(resolved.configuration.env_name)),
    ("method", Json::string(method_name)),
  ]
  match resolved.phantom_id {
    Some(id) => selector.push(("phantomId", Json::string(id)))
    None => ()
  }
  let frozen_start = protocol_message("invocationStart", [
    ("attemptId", Json::string(attempt_id)),
    (
      "config",
      Json::array(
        resolved.config.map(entry => {
          obj([
            ("path", Json::array(entry.path.map(Json::string))),
            ("value", entry.codec.encode(entry.value)),
          ])
        }),
      ),
    ),
    ("idempotencyKey", Json::string(idempotency_key)),
    ("methodParameters", input_codec.encode(method_parameters)),
    ("selector", obj(selector)),
  ])
  let (socket, accepted) = connect_start_exact_retry(url, headers, frozen_start)
  match expect_string(get_field(accepted, "type")) {
    "invocationRejected" => {
      match field_opt(accepted, "attemptId") {
        Some(value) if expect_string(value) != attempt_id =>
          raise BridgeError(
            "attempt-conflict: rejection attempt does not match",
          )
        _ => ()
      }
      raise BridgeError(expect_string(get_field(accepted, "message")))
    }
    "invocationAccepted" => ()
    _ => raise BridgeError("protocol-error: invalid start response")
  }
  validate_acceptance_identity(accepted, attempt_id, idempotency_key)
  let session = {
    socket,
    url,
    headers,
    session_token: None,
    attempt_id,
    idempotency_key,
    inputs,
    input_channels: {},
    input_tokens: {},
    next_input_index: 0,
    input_replay_bytes: 0,
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
      receive_actor(session, resolved) catch {
        error => fail_session(session, error)
      }
    }),
  )
  session.result_queue.get()
}
