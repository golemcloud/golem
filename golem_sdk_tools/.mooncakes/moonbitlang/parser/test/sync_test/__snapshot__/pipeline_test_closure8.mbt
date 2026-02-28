fn h(f: () -> Int) -> Unit {
  println(f())
}

fn g(a: Int) -> Int {
  fn f() -> Int {
    a
  }

  h(f)
  println("xx")
  f()
}

fn main {
  println(g(42))
}
