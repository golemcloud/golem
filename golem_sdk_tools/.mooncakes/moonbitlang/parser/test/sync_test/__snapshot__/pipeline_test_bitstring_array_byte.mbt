//!build: -w -27

///|
fn to_be_bytes(n : UInt) -> Bytes {
  [
    (n >> 24).to_byte(),
    (n >> 16).to_byte(),
    (n >> 8).to_byte(),
    n.to_byte(),
  ]
}

///|
fn to_be_bytes_u64(n : UInt64) -> Bytes {
  [
    (n >> 56).to_byte(),
    (n >> 48).to_byte(),
    (n >> 40).to_byte(),
    (n >> 32).to_byte(),
    (n >> 24).to_byte(),
    (n >> 16).to_byte(),
    (n >> 8).to_byte(),
    n.to_byte(),
  ]
}

///|
fn f(x : Array[Byte]) -> Unit {
  match x {
    [u8(a), u16(b), u32(c), u64(d), bytes(16, bs), .. rest] => {
      println(to_be_bytes(a))
      println(to_be_bytes(b))
      println(to_be_bytes(c))
      println(to_be_bytes_u64(d))
      println(bs)
      println(rest)
    }
    _ => ()
  }
}

///|
fn g(x : ArrayView[Byte]) -> Unit {
  match x {
    [u8le(a), u16le(b), u32le(c), u64le(d), bytes(16, bs), .. rest] => {
      println(to_be_bytes(a))
      println(to_be_bytes(b))
      println(to_be_bytes(c))
      println(to_be_bytes_u64(d))
      println(bs)
      println(rest)
    }
    _ => ()
  }
}

///|
fn main {
  let arr : Array[Byte] = Array::makei(36, i => i.to_byte())
  f(arr)
  let arr2 : ArrayView[Byte] = arr[:]
  g(arr2)
}
