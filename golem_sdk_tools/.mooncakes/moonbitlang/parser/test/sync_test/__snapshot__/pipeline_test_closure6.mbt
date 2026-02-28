fn r1(x: Int) -> Int {
  fn rec(y: Int) {
    fn inner() {
      fn inner2(a: Int) {
        x + y + a
      }
      inner2(1) + rec(y - 1)
    }
    if y == 0 { 0 } else { 1 + inner() }
  }
  1 + rec(x)
}

fn init {
  println(r1(2))
}
