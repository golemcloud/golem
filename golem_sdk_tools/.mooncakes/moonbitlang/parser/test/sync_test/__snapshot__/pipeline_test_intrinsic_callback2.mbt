///|
trait Add {
  f(x : Self, y : Int) -> Int // currently it still works without Self type
}

///|
impl Add for Int with f(x, y) {
  x + y
}
impl Add for String with f(_, y) {
  y
}
///|
fn id(x : (&Add, Int) -> Int) -> (&Add, Int) -> Int {
  x
}

///|
fn main {
  println(id(&Add::f)(1, 2))
  println(id(&Add::f)(3, 4))
  println(id(&Add::f)("hello", 2))
}
