fn g() -> Unit {
  fn h(x) {
    guard x != 0 else { return }
    println(x)
    h(x-1)
  }
  let _ = h(3)
}

fn main {
  g()
}
