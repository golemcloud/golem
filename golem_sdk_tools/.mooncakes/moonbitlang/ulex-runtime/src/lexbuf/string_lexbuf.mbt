///|
pub(open) trait IStringLexbuf {
  next_as_int(Self) -> Int
  curr_pos(Self) -> Int
  reset(Self, pos~ : Int) -> Unit
  get_char(Self, Int, Int) -> Char
  get_string(Self, Int, Int) -> String
}

///|
pub struct StringLexbuf {
  data : String
  end : Int
  mut pos : Int
}

///|
pub fn StringLexbuf::from_string(string : String) -> StringLexbuf {
  StringLexbuf::{ data: string, end: string.length(), pos: 0 }
}

///|
pub impl IStringLexbuf for StringLexbuf with next_as_int(self) {
  let pos = self.pos
  let end = self.end
  if self.pos < end {
    let c = self.data.unsafe_charcode_at(pos)
    if c >= 0xD800 && c <= 0xDBFF {
      let pos2 = pos + 1
      if pos2 < end {
        let c2 = self.data.unsafe_charcode_at(pos2)
        if c2 >= 0xDC00 && c2 <= 0xDFFF {
          self.pos = pos2 + 1
          return (c - 0xD800) * 0x400 + c2 - 0xDC00 + 0x10000
        }
      }
    }
    self.pos = pos + 1
    return c
  }
  -1
}

///|
pub impl IStringLexbuf for StringLexbuf with curr_pos(self) {
  self.pos
}

///|
pub impl IStringLexbuf for StringLexbuf with reset(self, pos~) {
  self.pos = pos
}

///|
pub impl IStringLexbuf for StringLexbuf with get_char(self, start, _end) {
  self.data.unsafe_char_at(start)
}

///|
pub impl IStringLexbuf for StringLexbuf with get_string(self, start, end) {
  self.data.substring(start~, end~)
}
