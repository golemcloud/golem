///|
fn f(x : UInt16) -> Unit {
  println(x)
}

///|
fn main {
  f('a')
  f('中')
  f('\u{0041}')
}
