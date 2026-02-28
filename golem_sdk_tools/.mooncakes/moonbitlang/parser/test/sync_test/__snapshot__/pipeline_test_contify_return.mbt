fn triple(x: Int) -> Int {
  let s = {
    let double = fn(x) { return x * 2 }
    double(x)
  }
  s + x
}

fn init {
  println(triple(16))
}
