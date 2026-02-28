fn init {
  let t = T(42)
  t.f((,) => println(21))

}

fn init {
  t.g((x,,) => println(x))

}

fn init {
  t.g(x + y => println(x))
}

fn init {
  t.g((x + y: Int) => println(x))

}

fn init {
  t.h((x + y, y) => println(x+y))
}

fn init {
  t.h((x: Int, y + z) => println(x+y))
}

fn init {
  t.h((x, y,,) => println(x+y))
}
