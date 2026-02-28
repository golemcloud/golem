///|
fn[T : Default + Show] f(_a : T) -> Unit {
  struct U(T) derive(Default, Show)
  let u1 = U::default()
  println(u1)
  let u2 : U = Default::default()
  println(u2)
}

///|
fn main {
  f(1)
  f(true)
}
