fn f(x: Int) -> Int {
  {
    fn g(x: Int) -> Int {
      x + 1
    }
    if x == 0 {
      return g(x)
    } else {
      g(x)
    }
  } +
  42
}

fn main {
  println(f(0))
}
