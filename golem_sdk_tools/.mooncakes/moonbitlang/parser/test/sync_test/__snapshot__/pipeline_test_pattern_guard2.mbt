fn f(a: Int?, b: Int?, c: Int?) -> Unit {
  match (a, b, c) {
    (None, None, None) => println("all none")
    (Some(1..=10), Some(b), c) if c is Some(c) && b >= c => println("b >= c \{b} \{c}")
    _ if a is Some(x) && g(a, b, c) => println("g is true where a is \{x}")
    _ => println("else")
  }
}

///|
fn g(a : Int?, b : Int?, c : Int?) -> Bool {
  a == b && b == c
}

///|
fn main {
  f(None, None, None)
  f(Some(1), Some(3), Some(2))
  f(Some(1), Some(2), Some(3))
  f(Some(42), Some(42), Some(42))
}
