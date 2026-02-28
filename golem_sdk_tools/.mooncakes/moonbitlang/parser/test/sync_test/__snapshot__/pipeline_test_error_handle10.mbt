fn f(x : UInt64) -> UInt64 raise {
  if x == 0 {
    fail("x is zero")
  } else {
    x
  }
}

fn main {
  println(try? f(0))
  println(try? f(1))
}
