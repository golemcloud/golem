///|

///|
fn f1(x : Float) -> Unit {
  match x {
    0.0 => println("zero")
    1 => println("one")
    _ => println("not zero or one")
  }
}

///|
fn main {
  f1(0.0)
  f1(1)
  f1(1.1)
}
