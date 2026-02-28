///|
fn f(b : Bool) -> Int {
  try {
    if b {
      fail("err")
    } else {
      return 42
    }
  } catch {
    _ => 0
  }
}

///|
fn main {
  println(f(true))
  println(f(false))
}
