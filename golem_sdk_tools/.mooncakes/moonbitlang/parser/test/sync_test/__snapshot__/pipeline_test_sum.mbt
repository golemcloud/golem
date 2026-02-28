fn sum(x: Int) -> Int {
  fn go(x: Int, s: Int) {
    if x == 0 { s } else { go(x-1, x+s) }
  }
  if x > 0 { go(x, 0) } else { 0 }
}

fn init {
  println(sum(10))
}
