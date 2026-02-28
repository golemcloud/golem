
fn[X] f(x: X) -> X {
  fn g(x) {}
  x
}

fn init {
  println(f(0))
}


enum T[X] {
  A
  B(X)
}
