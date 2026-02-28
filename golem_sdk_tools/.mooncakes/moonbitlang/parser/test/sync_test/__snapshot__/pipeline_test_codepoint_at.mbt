///|
#intrinsic ("%string.codepoint_at")
fn codepoint_at(s : String, i : Int) -> Int {
  let code = s.code_unit_at(i)
  if code >= 0xD800 && code <= 0xDBFF {
    let next = s.code_unit_at(i + 1)
    if next >= 0xDC00 && next <= 0xDFFF {
      return ((code - 0xD800) << 10).to_int() + (next - 0xDC00).to_int() + 0x10000
    } else {
      abort("invalid string")
    }
  } else {
    code.to_int()
  }
}

///|
fn main {
  let s = "😭1😭"
  let c1 = codepoint_at(s, 0).unsafe_to_char()
  let c2 = codepoint_at(s, 2).unsafe_to_char()
  let c3 = codepoint_at(s, 3).unsafe_to_char()
  println(c1)
  println(c2)
  println(c3)
  let a1 = codepoint_at(s, 1)
  let a2 = codepoint_at(s, 4)
  println(a1)
  println(a2)
}
