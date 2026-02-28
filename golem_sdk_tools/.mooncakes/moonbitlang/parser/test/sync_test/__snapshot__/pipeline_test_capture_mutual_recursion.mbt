///|
pub fn f() -> Unit {
  let mut x = 10
  letrec f: (Int) -> Int = n1 => {
    x = 42
    if n1 < 0 {
      0
    } else {
      g(n1)
    }
  }
  and g: (Int) -> Int = n2 => f(n2)

  fn h(x : Int) -> Int {
    g(x)
  }

  println(h(-1))
  println(h(-1))
}

///|
fn main {
  f()
}
