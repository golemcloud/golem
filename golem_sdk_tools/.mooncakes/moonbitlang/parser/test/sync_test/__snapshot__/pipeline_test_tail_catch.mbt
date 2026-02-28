///|
fn r(b : Bool) -> Int raise {
  if b {
    fail("rrr")
  }
  42
}

///|
fn f(b1 : Bool, b2 : Bool) -> Int raise {
  try {
    r(b1)
  } catch {
    _ => if b2 { fail("fff") } else { 24 }
  }
}

///|
fn main {
  println(try? f(true, true))
  println(try? f(false, false))
  println(try? f(true, false))
  println(try? f(false, true))
}
