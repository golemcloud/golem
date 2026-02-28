fn triple(x: Int) -> Int {
  let triple = fn(x) { return x * 3 }
  triple(x)
}

fn init {
  println(triple(16))
}
