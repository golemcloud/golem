fn[T : Compare] hey(x : T, y : T) -> Int {
  x.compare(y)
}

fn init {
  println(hey(1, 2))
  println(hey(2, 1))
  println(hey(2, 2))
  println(hey(1.0, 2.0))
  println(hey(2.0, 1.0))
  println(hey(2.0, 2.0))
  println(hey("a", "ab"))
  println(hey("ab", "a"))
  println(hey("ab", "ba"))
}

fn f() -> Bool {
  fn g(x, y) {
    x >= y
  }
  g(1,2)
}

fn init {
  if f() { println("not ok") } else { println("ok") }
}
