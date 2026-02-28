fn f(a: Int) -> Int {
  let mut ma = a
  while ma > 10 {
    if ma > 100 { return ma }
    ma = ma + 1
  }
  return a
}

fn g(a: Int) -> Unit {
  if a == 1 { println("value is one\n"); return  }
  if a == 2 { println("value is two\n"); return  }
  println("value is neither one nor two\n")
}

fn h(a: Int) -> Unit {
  if a == 1 { return g(a) }
  return println(f(a))
}

fn init {
  println(f(10))
  println(f(11))

  g(1)
  g(2)
  g(3)

  h(1)
  h(2)
}

