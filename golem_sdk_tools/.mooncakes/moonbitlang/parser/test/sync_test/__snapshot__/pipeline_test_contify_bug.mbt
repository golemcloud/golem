
fn f() -> Int {
  fn local_fn() { true }
  if local_fn() {
    100
  } else {
    200
  }
}

fn g() -> Unit {
  fn local_fn() { println("!") }
  for i = 0; i < 2; i = i + 1 {
    local_fn()
  }
}

fn init {
  println(f())
  g()
}
