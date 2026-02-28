//|
pub fn[T : Show] f(x : T) -> Unit {
  struct A(T) derive(Show)
  let a = A(x)
  println(a)
}

///|
fn main {
  f(42)
}
