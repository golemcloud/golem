///|
fn main {
  let x : Bytes = [b'\x00']
  let z = make_bytes(42)
  println(x)
  println(z)
}

///|
fn make_bytes(x : Byte) -> Bytes {
  [x, x]
}
