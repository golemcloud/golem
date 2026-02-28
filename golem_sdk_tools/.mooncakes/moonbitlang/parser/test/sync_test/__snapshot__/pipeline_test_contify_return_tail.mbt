fn init {
  println(f(100))
}

fn f(x: Int) -> Int {
  fn aux(n: Int) {
    if n == 0 {
      0
    } else {
      if n > 10000 {
        return -1
      }
      aux(n - 1)
    }
  }
  aux(x)
}

