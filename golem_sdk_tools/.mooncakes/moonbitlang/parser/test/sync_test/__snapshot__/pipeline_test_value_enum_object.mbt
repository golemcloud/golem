///|
#valtype
enum E {
  A
  B(Int)
  C(Double, Double)
} derive(Show)

///|
fn use_show(x : &Show) -> Unit {
  println(x.to_string())
}

///|
fn main {
  let e = E::A
  use_show(e)
  let e2 = E::B(1)
  use_show(e2)
  let e3 = E::C(1.0, 2.0)
  use_show(e3)
}
