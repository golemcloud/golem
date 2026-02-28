///|
fn fxxx(x : FixedArray[Int16]) -> Int16? {
  Some(x.unsafe_get(0))
}

///|
fn main {
  println(fxxx([-32768]))
}
