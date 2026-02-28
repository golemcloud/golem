struct S1 {
  f: (S2, E1) -> Unit
  g: (S2, E2) -> Unit
}

struct S2 {
  f: S1
}

enum E1 {
  A(Int)
}

enum E2 {
  A(Int)
}

fn make_s1(f: (S2, E1) -> Unit, g: (S2, E2) -> Unit) -> S1 {
  S1::{f, g}
}

fn t(a: Int) -> Unit {
  fn g(x: S2, y: E2) -> Unit {
    ignore(a)
  }
  fn f(x: S2, y: E1) -> Unit {
    ignore(a)
  }

  let _ = make_s1(f, g)
}

fn main {
  t(42)
}
