///|
fn f(x : ReadOnlyArray[(String, String)]) -> Unit {
  println(x)
}

///|
fn main {
  f([("x1", "y1"), ("x2", "y2")])
}
