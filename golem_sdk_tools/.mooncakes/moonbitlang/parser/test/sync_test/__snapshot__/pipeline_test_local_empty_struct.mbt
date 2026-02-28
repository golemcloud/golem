///|
fn main {
  struct S {} derive(Eq)
  let a = S::{  }
  let b = S::{  }
  println(a == b)
}
