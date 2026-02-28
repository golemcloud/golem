fn down_to_0(x: Int, f: (Int) -> Unit) -> Unit {
  if x >= 0 {
    f(x)
    down_to_0(x - 1, f)
  }
}

fn init {
  down_to_0(10, fn(x) {
    println(x)

  })
}
