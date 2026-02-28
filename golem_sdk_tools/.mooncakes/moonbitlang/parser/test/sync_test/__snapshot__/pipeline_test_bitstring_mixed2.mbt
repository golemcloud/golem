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
const B : Byte = 0x10

///|
const C : Byte = 0xFF

///|
const Bs : Bytes = "\x10\x10\x10\x10"

///|
fn f(bs : Bytes) -> Unit {
  match bs {
    [B | C, u1(a), u7(b), .. Bs, u32(c), .. Bs, 0xa0..=0xb0, .. rest] => {
      println(a)
      println(b)
      println(to_be_bytes(c))
      println(rest)
    }
    _ => println("other")
  }
}

///|
fn g(bs : Bytes) -> Unit {
  match bs {
    [.. "hello", .. b"world", u16(c), u32(d), ..] => {
      println(to_be_bytes(c))
      println(to_be_bytes(d))
    }
    _ => println("other")
  }
}

///|
fn main {
  let bs1 = b"\x10\xFF\x10\x10\x10\x10\x12\x34\x56\x78\x10\x10\x10\x10\xab\xcd"
  let bs2 = b"\xFF\xFF\x10\x10\x10\x10\x12\x34\x56\x78\x10\x10\x10\x10\xb0\xcd"
  let bs3 = b"\xFF\xFF\x10\x10\x10\x10\x12\x34\x56\x78\x10\x10\x10\x10\xcd\xab"
  f(bs1)
  f(bs2)
  f(bs3)
  g(b"helloworldabcde")
  g(b"helloworldabcdef")
  g(b"abcdefhelloworld")
}
