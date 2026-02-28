///|
pub(open) trait IBytesLexbuf {
  next_as_int(Self) -> Int
  curr_pos(Self) -> Int
  reset(Self, pos~ : Int) -> Unit
  get_char(Self, Int, Int) -> Byte
  get_string(Self, Int, Int) -> @bytes.View
}

///|
pub struct BytesLexbuf {
  data : Bytes
  end : Int
  mut pos : Int
}

///|
pub fn BytesLexbuf::from_bytes(bytes : Bytes) -> BytesLexbuf {
  BytesLexbuf::{ data: bytes, end: bytes.length(), pos: 0 }
}

///|
pub impl IBytesLexbuf for BytesLexbuf with next_as_int(self) {
  if self.pos >= self.end {
    return -1
  }
  let byte = self.data[self.pos]
  self.pos += 1
  return byte.to_int()
}

///|
pub impl IBytesLexbuf for BytesLexbuf with curr_pos(self) {
  return self.pos
}

///|
pub impl IBytesLexbuf for BytesLexbuf with reset(self, pos~ : Int) {
  self.pos = pos
}

///|
pub impl IBytesLexbuf for BytesLexbuf with get_char(
  self,
  start : Int,
  _end : Int
) {
  self.data[start]
}

///|
pub impl IBytesLexbuf for BytesLexbuf with get_string(
  self,
  start : Int,
  end : Int
) {
  return self.data[start:end]
}
