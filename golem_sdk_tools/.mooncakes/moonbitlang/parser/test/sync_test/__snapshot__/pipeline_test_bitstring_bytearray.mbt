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
enum E {
  A(FixedArray[Byte])
  B(ReadOnlyArray[Byte])
  C(Array[Byte])
  D(ArrayView[Byte])
  E(Bytes)
  F(BytesView)
}

///|
fn f(e : E) -> Unit {
  match e {
    A([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    B([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    C([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    D([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    E([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    F([u8be(a), .. rest, u16le(b)] as c) => {
      println(a)
      println(to_be_bytes(b))
      println(rest)
      println(c)
    }
    _ => println("other")
  }
}

///|
fn main {
  f(A([0x01, 0x02, 0x03, 0x04]))
  f(B([0x05, 0x06, 0x07, 0x08]))
  f(C([0x09, 0x0A, 0x0B, 0x0C]))
  f(D([0x0D, 0x0E, 0x0F, 0x10]))
  f(E([0x11, 0x12, 0x13, 0x14]))
  f(F([0x15, 0x16, 0x17, 0x18]))
  f(F([]))
}
