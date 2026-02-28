///|
pub fn f(x : Bool) -> Bool {
  fn g() -> Bool {
    not(x)
  }

  if x {
    x && g()
  } else {
    x || g()
  }
}

///|
fn main {
  println(f(true))
}
