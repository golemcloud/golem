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
    A([i1be(a), i2be(b), i5be(c), ..]) => {
      println(a)
      println(b)
      println(c)
    }
    B([i8be(a), ..]) => println(a)
    C([i12be(a), i12be(b), ..]) => {
      println(a)
      println(b)
    }
    D([i32be(a), i32le(b), ..]) => {
      println(a)
      println(b)
    }
    E([i64be(a), i64le(b), ..]) => {
      println(a)
      println(b)
    }
    _ => println("other")
  }
}

///|
fn main {
  f(A([0xF0]))
  f(B([0xA0]))
  f(C([0xA0, 0xA0, 0xA0]))
  f(D([0xFF, 0xFF, 0xFF, 0xFE, 0xFE, 0xFF, 0xFF, 0xFF]))
  f(
    E([
      0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF,
      0xFF, 0xFF, 0xFF,
    ]),
  )
}
