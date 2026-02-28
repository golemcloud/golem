//!build: -w -28
fn f() -> Unit {
  ...
}

fn g() -> Bool? {
  f()
  ...
}

fn init {
  println(g())
}
