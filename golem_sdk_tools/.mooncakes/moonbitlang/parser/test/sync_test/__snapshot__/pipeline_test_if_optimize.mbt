///|
fn f() -> Bool {
  println("hello")
  true
}

///|
fn g() -> Int {
  42
}

///|
fn main {
  let a = if f() { g() } else { g() }
  println(a)
}
