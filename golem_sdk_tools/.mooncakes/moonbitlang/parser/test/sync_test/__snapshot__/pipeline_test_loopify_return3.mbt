///|
fn hof(f : (Int) -> Int) -> Int {
  f(10)
}

///|
fn loop2() -> Int {
  fn f(x : Int) {
    hof(
        a => {
          if a > 0 {
            ()
          } else {
            return f(42)
          }
          x
        }
    )
  }

  f(10) + f(-1)
}

///|
fn main {
  println(loop2())
}
