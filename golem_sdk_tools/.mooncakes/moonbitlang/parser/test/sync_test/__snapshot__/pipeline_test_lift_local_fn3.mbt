fn t(x : Int) -> Unit {
  let f = fn() -> Unit { println(x) }
  letrec g = fn(a : Int) -> Unit {
    f()
    if a > 0 {
      h(a - 1)
    }
  }

  and h = fn(a : Int) -> Unit {
    f()
    g(a)
  }

  g(1)
}

fn main {
  t(42)
}
