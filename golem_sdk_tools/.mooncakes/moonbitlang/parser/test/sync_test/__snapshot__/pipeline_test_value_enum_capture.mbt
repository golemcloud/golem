///|
#valtype
enum E {
  A
  B(Int)
  C(Double, Double)
} derive(Show)

///|
fn make_capture(e : E, i : Int) -> () -> String {
  () => e.to_string() + i.to_string()
}

///|
fn main {
  let e = E::A
  let f = make_capture(e, 1)
  println(f())
  let f2 = make_capture(E::B(1), 2)
  println(f2())
  let f3 = make_capture(E::C(1.0, 2.0), 3)
  println(f3())
}
