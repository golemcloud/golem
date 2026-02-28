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
fn f(bs : BytesView) -> Unit {
  match bs {
    [.. rest, u4(a), u2(b), u1(c), u1(d)] => {
      println(a)
      println(b)
      println(c)
      println(d)
      println(rest)
    }
    _ => println("other")
  }
}

///|
fn g(bs : BytesView) -> Unit {
  match bs {
    [u32(a), .. rest, u32le(b)] => {
      println(to_be_bytes(a))
      println(to_be_bytes(b))
      println(rest)
    }
    _ => println("other")
  }
}

///|
fn h(bs : BytesView) -> Unit {
  match bs {
    [a, .. rest, '\x00', u32be(b), c] => {
      println("first")
      println(a)
      println(to_be_bytes(b))
      println(c)
      println(rest)
    }
    [a, .. rest, '\x0A', u32be(b), c] => {
      println("second")
      println(a)
      println(to_be_bytes(b))
      println(c)
      println(rest)
    }
    _ => println("other")
  }
}

///|
fn h1(bs : BytesView) -> Unit {
  match bs {
    [a, .. rest, u32be(b), .. b"\x0F\x0E"] => {
      println("first")
      println(a)
      println(to_be_bytes(b))
      println(rest)
    }
    [a, .. rest, u32be(b), .. b"\x0E\x0F"] => {
      println("second")
      println(a)
      println(to_be_bytes(b))
      println(rest)
    }
    _ => println("other")
  }
}

///|
fn main {
  let bs = b"\xFF\xAA"
  f(bs[:])
  f("")
  let bs = b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F"
  g(bs[:])
  g("")
  h(bs[:])
  h("")
  h1(bs[:])
  h1("")
}
