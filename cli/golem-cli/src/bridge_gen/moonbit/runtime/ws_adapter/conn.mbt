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
// we currently allow at most 2^16 bytes in a single frame when sending
const MAX_HEADER_SIZE : Int = 8 + 8 + 16

///|
const MASK_SIZE : Int = 4

///|
priv trait Transport: @io.Reader + @io.Writer {
  fn close(Self) -> Unit
}

///|
impl Transport for @http.Client with fn close(self) {
  self.close()
}

///|
extend &Transport with @io.Reader::{read, read_exactly}

///|
extend &Transport with @io.Writer::{write}

///|
/// A WebSocket connection
#warnings("-alert_internal")
struct Conn {
  mut closed : WebSocketError?
  transport : &Transport

  // =================
  // state for reading
  // =================
  read_buf : @io.ReaderBuffer

  // if `read_opcode` is `Some(opcode)`,
  // we are in the middle of a fragmented message,
  // and the last frame (i.e. the frame with `FIN = true`) has not been received yet.
  // if `read_opcode` is `None`,
  // the last frame of current message has been received,
  // but the last frame itself may not be completely consumed yet.
  mut read_opcode : OpCode?

  // the number of remaining bytes in the frame being processed.
  mut curr_frame_remaining : Int64

  // the message currently being received.
  // If `recv` is called while `curr_message` is present,
  // the current message must be discarded.
  mut curr_message : Message?

  // the mask of the frame being processed, if any
  mut read_mask : Bytes?

  // when perform masking on the receiving side,
  // user may request data in a non 4-byte aligned manner.
  // In this case, we use `mask_offset` to store current offset in the mask.
  mut mask_offset : Int

  // when receiving a text message,
  // we perform rolling UTF8 validation to confront to the protocol spec.
  // Sometimes maybe only a part of a complete UTF8 char have been received.
  // In this case, we use `utf8_remaining` to store
  // the remaining number of bytes expected in the current UTF8 character,
  // so that we can continue UTF8 validation when more data arrives.
  // `-1` indicates that current message is a binary message.
  mut utf8_remaining : Int

  // =================
  // state for writing
  // =================

  // In general, we require only one writer at any time in `moonbitlang/async`,
  // violating this principle is undefined behavior.
  // However, for WebSocket, the reader may trigger writing as well by:
  //
  // 1. closing the tunnel
  // 2. respond to a received `PING` message
  //
  // And we do allow reader & writer to run in parallel for the same tunnel.
  // So we need a lock to avoid race condition in writing.
  //
  // All modification to existing content of `write_buf`
  // and direct writing to `transport` must be protected by this lock.
  write_lock : @semaphore.Semaphore

  // If present, `write_mask` is a random generator used to generate frame masks.
  // If absent, frames will be unmasked.
  write_mask : @random.Rand?

  // We reserve some fixed space at the start of `write_buf` for header,
  // `payload_start` is the start offset of message payload in the buffer.
  payload_start : Int

  // The kind of the message currently being sent.
  // If the value is `None`, we are not in the middle of a message.
  mut write_opcode : OpCode?

  // Whether we are currently sending the first frame of current message.
  // If `false`, new frames should have op code `CONTINUATION`.
  mut is_first_frame : Bool

  // Buffer holding message content to write.
  // We perform auto fragmentation for WebSocket message:
  // as soon as `write_buf` is full,
  // we split the message and send a frame immediately.
  write_buf : FixedArray[Byte]

  // the length of *payload* currently buffered in `write_buf`
  mut write_len : Int
}

///|
priv enum MessageState {
  Active
  Closed
  Error(Error)
  Interrupted
}

///|
/// A message received from a WebSocket tunnel
pub struct Message {
  kind : MessageKind
  priv mut state : MessageState
  priv conn : Conn
}

///|
#warnings("-alert_internal")
fn Conn::new(
  transport : &Transport,
  max_frame_size : Int,
  mask~ : @random.Rand?,
) -> Conn {
  {
    closed: None,
    transport,
    read_buf: @io.ReaderBuffer::new(),
    read_opcode: None,
    curr_frame_remaining: 0,
    curr_message: None,
    read_mask: None,
    mask_offset: 0,
    utf8_remaining: -1,
    write_lock: @semaphore.Semaphore(1),
    write_mask: mask,
    payload_start: if mask is Some(_) {
      MAX_HEADER_SIZE + MASK_SIZE
    } else {
      MAX_HEADER_SIZE
    },
    write_opcode: None,
    is_first_frame: true,
    write_buf: FixedArray::make(max_frame_size, 0),
    write_len: 0,
  }
}

///|
/// Close a WebSocket connection.
/// The underlying HTTP connection will be closed as well.
pub fn Conn::close(self : Conn) -> Unit {
  if self.curr_message is Some(message) {
    message.state = Error(ConnectionClosed(Normal, None))
    self.curr_message = None
  }
  if self.closed is None {
    self.closed = Some(ConnectionClosed(Normal, None))
  }
  self.transport.close()
}

///|
/// Initiating the connection closing process of WebSocket.
/// Note that `send_close` merely performs closing at WebSocket protocol level, so:
///
/// - `Conn::close` should be called anyway, even if `send_close` is called
/// - `Conn::send_close` should only be called when everything goes well.
///    For example it should NOT be called when protocol error or network error occurs.
pub async fn Conn::send_close(
  self : Conn,
  code? : CloseCode = Normal,
  reason? : String,
) -> Unit {
  if self.closed is Some(err) {
    raise err
  } else {
    self.closed = Some(ConnectionClosed(code, reason))
    self.send_close_unchecked(code, reason?)
  }
}

///|
async fn Conn::flush_frame_unprotected(
  self : Conn,
  op_code : OpCode,
  fin~ : Bool,
) -> Unit {
  // mask the frame if necessary
  let header_end = if self.write_mask is Some(rand) {
    let mask = rand.uint()
    self.write_buf.unsafe_write_uint32_be(self.payload_start - 4, mask)
    let mask_bytes = FixedArray::make(MASK_SIZE, b'\x00')
    mask_bytes.unsafe_write_uint32_be(0, mask)
    mask_payload(
      self.write_buf,
      mask_bytes.unsafe_reinterpret_as_bytes(),
      offset=self.payload_start,
      len=self.write_len,
    )
    self.payload_start - 4
  } else {
    self.payload_start
  }

  // fill header
  let frame_start = if self.write_len > 125 {
    self.write_buf.unsafe_write_uint16_be(
      header_end - 2,
      self.write_len.to_uint16(),
    )
    header_end - 4
  } else {
    header_end - 2
  }
  let frame_end = self.payload_start + self.write_len
  let fin : Byte = if fin { 0x80 } else { 0 }
  let masked : Byte = if self.write_mask is None { 0 } else { 0x80 }
  self.write_buf[frame_start] = fin | op_code.to_byte()
  self.write_buf[frame_start + 1] = masked |
    @cmp.minimum(self.write_len, 126).to_byte()

  // actually send the frame
  let data = self.write_buf.unsafe_reinterpret_as_bytes()[frame_start:frame_end]
  self.transport.write(data)
  self.write_len = 0
}

///|
async fn Conn::send_close_unchecked(
  self : Conn,
  code : CloseCode,
  reason? : String,
) -> Unit {
  self.write_lock.acquire()
  defer self.write_lock.release()

  // discard current message and send the `close` frame
  self.write_buf.unsafe_write_uint16_be(self.payload_start, code.to_uint16())
  let reason_bytes = @utf8.encode(reason.unwrap_or(""))
  if reason_bytes.length() > 123 {
    // Close reason too long
    // TODO: should we close the connection anyway?
    fail("Close reason too long")
  }
  self.write_buf.blit_from_bytes(
    self.payload_start + 2,
    reason_bytes,
    0,
    reason_bytes.length(),
  )
  self.write_len = 2 + reason_bytes.length()
  self.flush_frame_unprotected(Close, fin=true)
}

///|
async fn[X] Conn::protocol_error(
  self : Conn,
  reason : String,
  code? : CloseCode = ProtocolError,
) -> X {
  let err : WebSocketError = ProtocolError(reason)
  if self.closed is None {
    self.closed = Some(err)
    self.send_close_unchecked(code, reason~)
  }
  raise err
}

///|
async fn Conn::recv_frame_header(self : Conn) -> FrameHeader {
  if self.closed is Some(err) {
    raise err
  }

  // Read first two bytes
  guard! self.transport.read_exactly(2)
    is [
      u1be(fin),
      u1be(rsv1),
      u1be(rsv2),
      u1be(rsv3),
      u4be(opcode),
      u1be(masked),
      u7be(payload_len),
    ]
  guard OpCode::from_byte(opcode.to_byte()) is Some(opcode) else {
    self.protocol_error("Invalid OP code \{opcode}")
  }
  let mut payload_len = payload_len.to_int64()

  // No extensions supported, rejecting frames with RSV bits set
  if rsv1 != 0 || rsv2 != 0 || rsv3 != 0 {
    self.protocol_error("Reserved bits are not zero", code=MissingExtension)
  }

  // Validate payload length according to RFC 6455 Section 5.5
  if opcode is (Close | Ping | Pong) {
    if fin == 0 {
      self.protocol_error("Control frames must not be fragmented")
    }
    if payload_len > 125L {
      self.protocol_error("Control frame too long", code=MessageTooBig)
    }
  }

  // Validate payload length according to RFC 6455 Section 5.2
  if payload_len == 126L {
    guard! self.transport.read_exactly(2) is [u16be(len)]
    payload_len = len.to_int64()
    if payload_len < 126L {
      self.protocol_error("Invalid payload length format")
    }
  } else if payload_len == 127L {
    guard! self.transport.read_exactly(8) is [u64be(len)]
    payload_len = len.reinterpret_as_int64()
    if payload_len < 0L {
      self.protocol_error("Invalid payload length format")
    }
    if payload_len < 65536L {
      self.protocol_error("Invalid payload length format")
    }
  }
  self.read_mask = if masked != 0 {
    self.mask_offset = 0
    Some(self.transport.read_exactly(4))
  } else {
    None
  }
  { opcode, fin: fin != 0, payload_len, }
}

///|
/// reply `PING` frame with a `PONG` frame
async fn Conn::handle_ping(self : Conn, frame : FrameHeader) -> Unit {
  // the protocol requires `payload_len <= 125`
  let len = frame.payload_len.to_int()
  let payload_start = if self.write_mask is Some(_) { 2 + MASK_SIZE } else { 2 }
  let pong_message = FixedArray::make(payload_start + len, b'\x00')
  pong_message[0] = 0x8a
  pong_message[1] = if self.write_mask is Some(_) {
    0x80 | len.to_byte()
  } else {
    len.to_byte()
  }
  for received = 0; received < len; {
    let n = self.transport.read(
      pong_message,
      offset=payload_start + received,
      max_len=len - received,
    )
    guard n > 0 else { self.protocol_error("Unexpected EOF") }
    continue received + n
  }
  if self.read_mask is Some(mask) {
    mask_payload(pong_message, mask, offset=payload_start, len~)
  }
  if self.write_mask is Some(rand) {
    pong_message.unsafe_write_uint32_be(2, rand.uint())
    let mask = pong_message.unsafe_reinterpret_as_bytes()[2:2 + MASK_SIZE]
    mask_payload(pong_message, mask, offset=payload_start, len~)
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  self.transport.write(pong_message.unsafe_reinterpret_as_bytes())
}

///|
async fn Conn::handle_pong(self : Conn, frame : FrameHeader) -> Unit {
  // the protocol requires `payload_len <= 125`
  let len = frame.payload_len.to_int()
  let msg = FixedArray::make(len, b'\x00')
  for received = 0; received < len; {
    let n = self.transport.read(msg, offset=received, max_len=len - received)
    guard n > 0 else { self.protocol_error("Unexpected EOF") }
    continue received + n
  }
  if self.read_mask is Some(mask) {
    mask_payload(msg, mask, offset=0, len~)
  }
  ignore(msg)
}

///|
async fn Conn::handle_close(self : Conn, frame : FrameHeader) -> Unit {
  guard self.closed is None else {
    // we are the one who initiate the close, ignore the server's response
    ()
  }
  let len = frame.payload_len.to_int()
  let payload_start = if self.write_mask is Some(_) { 2 + MASK_SIZE } else { 2 }
  let reply = FixedArray::make(payload_start + len, b'\x00')
  reply[0] = 0x88
  reply[1] = if self.write_mask is Some(_) {
    0x80 | len.to_byte()
  } else {
    len.to_byte()
  }
  for received = 0; received < len; {
    let n = self.transport.read(
      reply,
      offset=payload_start + received,
      max_len=len - received,
    )
    guard n > 0 else { self.protocol_error("Unexpected EOF") }
    continue received + n
  }
  if self.read_mask is Some(mask) {
    mask_payload(reply, mask, offset=payload_start, len~)
  }
  let payload = reply.unsafe_reinterpret_as_bytes()[payload_start:]
  let (close_code, reason) = if payload is [u16be(code), .. reason] {
    let reason = @utf8.decode(reason) catch {
      _ =>
        self.protocol_error(
          "Invalid UTF8 in close reason",
          code=InvalidFramePayload,
        )
    }
    let reason = if reason.length() is 0 { None } else { Some(reason) }
    (CloseCode::from_uint(code), reason)
  } else {
    // the `close` packet we receive is empty
    (CloseCode::Normal, None)
  }
  self.closed = Some(ConnectionClosed(close_code, reason))
  if self.write_mask is Some(rand) {
    reply.unsafe_write_uint32_be(2, rand.uint())
    let mask = reply.unsafe_reinterpret_as_bytes()[2:2 + MASK_SIZE]
    mask_payload(reply, mask, offset=payload_start, len~)
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  self.transport.write(reply.unsafe_reinterpret_as_bytes())
}

///|
pub async fn Conn::recv(self : Conn) -> Message {
  if self.curr_message is Some(msg) {
    while msg.drop(1024) == 1024 {
      ()
    }
    if msg.state is (Active | Closed) {
      msg.state = Interrupted
    }
  }
  let kind : MessageKind = for frame = self.recv_frame_header() {
    match frame.opcode {
      Close => {
        self.handle_close(frame)
        continue self.recv_frame_header()
      }
      Ping => {
        self.handle_ping(frame)
        continue self.recv_frame_header()
      }
      Pong => {
        self.handle_pong(frame)
        continue self.recv_frame_header()
      }
      Continuation => self.protocol_error("Unexpected continuation frame")
      Text => {
        if !frame.fin {
          self.read_opcode = Some(Text)
        }
        self.curr_frame_remaining = frame.payload_len
        self.utf8_remaining = 0
        break Text
      }
      Binary => {
        if !frame.fin {
          self.read_opcode = Some(Binary)
        }
        self.curr_frame_remaining = frame.payload_len
        self.utf8_remaining = -1
        break Binary
      }
    }
  }
  let message = { kind, conn: self, state: Active, }
  self.curr_message = Some(message)
  message
}

///|
pub impl @io.Reader for Message with fn _get_internal_buffer(self) {
  self.conn.read_buf
}

///|
#warnings("-fragile_catch_all")
pub impl @io.Reader for Message with fn _direct_read(
  self,
  buf,
  offset~,
  max_len~,
) {
  match self.state {
    Active => ()
    Closed => return 0
    Error(err) => raise err
    Interrupted =>
      abort(
        "A new message is requested before current message is completely read, there is probably a race condition bug in user code",
      )
  }
  let conn = self.conn
  if conn.closed is Some(err) {
    raise err
  }
  while conn.curr_frame_remaining == 0 {
    // if we are not in the last frame of a message,
    // the end of this frame is the end of the whole message
    if conn.read_opcode is None {
      self.state = Closed
      conn.curr_message = None
      return 0
    }

    // receive the continuation frames of a fragmented message
    try {
      let frame = conn.recv_frame_header()
      match frame.opcode {
        Continuation => {
          if frame.fin {
            // inidicate there is no more frame for this message
            conn.read_opcode = None
          }
          // continue with the next frame
          conn.curr_frame_remaining = frame.payload_len
        }
        Text | Binary =>
          // receiving a new message in the middle of
          // a fragmented message is not supported
          conn.protocol_error("New message within a fragmented message")
        Ping => conn.handle_ping(frame)
        Pong => conn.handle_pong(frame)
        Close => conn.handle_close(frame)
      }
    } catch {
      err => {
        self.state = Error(err)
        raise err
      }
    }
  }

  // we have a non-empty frame pending, read from that frame
  let max_len = @cmp.minimum(conn.curr_frame_remaining, max_len.to_int64()).to_int()
  let n = conn.transport.read(buf, offset~, max_len~) catch {
    err => {
      self.state = Error(err)
      raise err
    }
  }
  conn.curr_frame_remaining -= n.to_int64()
  if conn.read_mask is Some(mask) {
    mask_payload(buf, mask, offset~, len=n, mask_offset=conn.mask_offset)
    conn.mask_offset = (conn.mask_offset + n) % 4
  }
  if conn.utf8_remaining >= 0 {
    conn.utf8_remaining = verify_utf8(
      buf.unsafe_reinterpret_as_bytes()[offset:offset + n],
      remaining=conn.utf8_remaining,
    ) catch {
      _ =>
        conn.protocol_error(
          "Text message is not valid UTF8",
          code=InvalidFramePayload,
        )
    }
  }
  n
}

///|
pub extend Message with @io.Reader::{
  read,
  drop,
  read_exactly,
  read_some,
  read_all,
  read_until,
}

///|
/// Start sending a new message to the server.
/// The content of the message can be sent by using the `self` as a `@io.Writer`
/// after calling `start_message`.
/// `end_message` must be explicitly called to terminate the message.
///
/// Writing message content is buffered.
/// To ensure immediate delivery of data,
/// split them into multiple messages,
/// and use `end_message` to ensure data is actually sent to the server.
///
/// `start_message` must NOT be called before the last message ends.
pub fn Conn::start_message(self : Conn, kind : MessageKind) -> Unit {
  guard self.write_opcode is None else {
    abort("cannot start a new message before the last message end")
  }
  self.write_opcode = Some(
    match kind {
      Text => Text
      Binary => Binary
    },
  )
  self.is_first_frame = true
}

///|
/// End the message currently being sent,
/// flush all buffered data and tell the server the termination of current message.
///
/// `end_message` must be called after the `start_message`.
pub async fn Conn::end_message(self : Conn) -> Unit {
  guard self.write_opcode is Some(opcode) else {
    abort("`end_message` called outside a message")
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  self.write_opcode = None
  if self.is_first_frame {
    self.flush_frame_unprotected(opcode, fin=true)
  } else {
    self.flush_frame_unprotected(Continuation, fin=true)
  }
}

///|
/// Flush buffered content in current message,
/// ensure that all currently written content are immediately delivered to the peer.
async fn Conn::flush(self : Conn) -> Unit {
  guard self.write_len > 0 else {  }
  guard self.write_opcode is Some(opcode) else {
    abort("writing to WebSocket outside a message")
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  if self.is_first_frame {
    self.flush_frame_unprotected(opcode, fin=false)
    self.is_first_frame = false
  } else {
    self.flush_frame_unprotected(Continuation, fin=false)
  }
}

///|
/// Write content to the current message being sent.
/// The message may be fragmented into multiple WebSocket frames automatically.
/// Writing message content is buffered.
/// To ensure immediate delivery of data, call `flush` manually.
pub impl @io.Writer for Conn with fn write_once(self, buf, offset~, len~) {
  if self.payload_start + self.write_len >= self.write_buf.length() {
    self.flush()
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  let write_buf_offset = self.payload_start + self.write_len
  let len = @cmp.minimum(len, self.write_buf.length() - write_buf_offset)
  self.write_buf.blit_from_bytes(write_buf_offset, buf, offset, len)
  self.write_len += len
  len
}

///|
pub extend Conn with @io.Writer::{write_once, write, write_reader}

///|
/// Convenient helper for sending a single text message.
/// To send large message lazily, see `start_message`.
pub async fn Conn::send_text(self : Conn, text : StringView) -> Unit {
  self..start_message(Text)..write(text).end_message()
}

///|
/// Convenient helper for sending a single binary message.
/// To send large message lazily, see `start_message`.
pub async fn Conn::send_binary(self : Conn, data : BytesView) -> Unit {
  self..start_message(Binary)..write(data).end_message()
}

///|
/// Send a PING frame to the peer, and wait for PONG reply.
/// If `msg` is provided, its content will become the body of the PING message.
/// Otherwise, `ping()` automatically generate random bytes as message body.
///
/// The PONG reply may not come immediately.
/// In particular, it may arrive after several other messages already on the wire.
/// To avoid data loss and race condition,
/// The adapter does not expose PONG synchronization because the generated
/// runtime does not use application-level WebSocket pings.
#warnings("-alert_internal")
pub async fn Conn::ping(self : Conn, msg? : BytesView) -> Unit {
  let msg = match msg {
    Some(msg) => msg
    None => @tls.rand_bytes(64)
  }
  let len = msg.length()
  guard! len <= 125
  let payload_start = if self.write_mask is Some(_) { 2 + MASK_SIZE } else { 2 }
  let frame = FixedArray::make(payload_start + len, b'\x00')
  frame[0] = 0x89
  frame[1] = if self.write_mask is Some(_) {
    0x80 | len.to_byte()
  } else {
    len.to_byte()
  }
  frame.blit_from_bytesview(payload_start, msg)
  if self.write_mask is Some(rand) {
    frame.unsafe_write_uint32_be(2, rand.uint())
    let mask = frame.unsafe_reinterpret_as_bytes()[2:2 + MASK_SIZE]
    mask_payload(frame, mask, offset=payload_start, len~)
  }
  self.write_lock.acquire()
  defer self.write_lock.release()
  self.transport.write(frame.unsafe_reinterpret_as_bytes())
}
