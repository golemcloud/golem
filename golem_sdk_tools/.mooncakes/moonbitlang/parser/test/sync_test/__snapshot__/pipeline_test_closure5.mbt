fn f(x: Int, y: Int) -> Int {
  fn g(n : Int) -> Int {
    n + x + y
  }
  let r = g(42) + g(42)
  fn h(m: Int) -> Int {
    g(m)
  }
  let p = h(42) + h(42)
  p + r
}

fn init {
  println(f(0, 1))
}
