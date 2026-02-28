//!build: -w -57

///|
pub(all) enum E {
  A(Int)
  B(i~ : Int, Int)
  C(i~ : Int, j~ : Int)
} derive(Show)

///|
pub fn f(e : E) -> Unit {
  match e {
    A as a => println(a)
    B as b => println(b.i)
    C as c => println(c.i + c.j)
  }
}

///|
fn main {
  f(A(1))
  f(B(1, i=2))
  f(C(i=1, j=2))
}
