fn hello(x: Int) -> Int {
  fn hey(y: Int) {
    y + x
  }

  hey(3)
}

fn init {
  println(hello(2))
}

