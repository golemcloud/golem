///|
fn bytes_contains(s : BytesView, c : Byte) -> Bool {
  for a in s {
    if a == c {
      return true
    }
  } else {
    false
  }
}

///|
const A : Bytes = "HELLO"

///|
fn main {
  println(bytes_contains("中文", '\xE4'))
  println(bytes_contains(A, 'L'))
  println(bytes_contains(Bytes::make(5, 'a'), 'e'))
  println(bytes_contains(['A', ..A], 'e'))
}
