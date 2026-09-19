// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
#warnings("-alert_internal")
struct TestTransport {
  buffer : @io.ReaderBuffer
}

///|
#warnings("-alert_internal")
fn TestTransport::new() -> TestTransport {
  { buffer: @io.ReaderBuffer::new(), }
}

///|
impl @io.Reader for TestTransport with fn _get_internal_buffer(self) {
  self.buffer
}

///|
impl @io.Reader for TestTransport with fn _direct_read(
  _self,
  buf,
  offset~ : Int,
  max_len~ : Int,
) -> Int {
  if max_len == 0 {
    0
  } else {
    buf[offset] = b'*'
    1
  }
}

///|
impl @io.Writer for TestTransport with fn write_once(
  _self,
  _buf,
  offset~ : Int,
  len~ : Int,
) -> Int {
  ignore(offset)
  len
}

///|
impl Transport for TestTransport with fn close(_self) {

}

///|
async test "zero-length message read does not close the reader" {
  let transport = TestTransport::new()
  let conn = Conn::new(transport, 1024, mask=None)
  conn.curr_frame_remaining = 1L
  conn.read_opcode = None
  let message = { kind: Binary, state: Active, conn, }
  conn.curr_message = Some(message)

  let empty = FixedArray::make(0, b'\x00')
  inspect(message.read(empty, max_len=0), content="0")
  inspect(message.state is Active, content="true")

  let byte = FixedArray::make(1, b'\x00')
  inspect(message.read(byte), content="1")
  inspect(byte[0] == b'*', content="true")
}
