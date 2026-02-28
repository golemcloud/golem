///|
pub fn[T : Show] f(x : T) -> Unit {
  struct A(T) derive(Show)
  struct B(Int, A) derive(Show)
  let a = A(x)
  println(a)
  let b = B(1, a)
  println(b)
}

///|
fn main {
  f(42)
}
