///|
fn string_spread() -> Unit {
  let s = "hello🤣😂😍"
  let sv = s.view(start_offset=1, end_offset=7)
  let str : String = ['x', ..s, ..sv, '😭']
  println(str)
}

///|
fn bytes_spread() -> Unit {
  let bs = b"Hello"
  let bv = bs[1:3]
  let bb : Bytes = [b'x', ..bs, ..bv, b'!', 98]
  println(bb)
}

///|
fn upper_char(x : Char) -> Char {
  if x >= 'a' && x <= 'z' {
    (x.to_int() - 32).unsafe_to_char()
  } else {
    x
  }
}

///|
fn upper_view(x : StringView) -> String {
  match x {
    ['a'..='z' as c,  .. rest,  'a'..='z' as c2] =>
      [upper_char(c), ..rest, upper_char(c2)]
    _ => [..x]
  }
}

///|
fn main {
  string_spread()
  bytes_spread()
  println(upper_view("hello".view()))
  println(upper_view("World".view()))
}
