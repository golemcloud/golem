///|
enum E {
  T1(Int, Int)
} derive(Show)

///|
struct K1(E) derive(Show)

///|
fn K1::f(self : K1) -> Unit {
  match self {
    K1(T1(0, 0)) => ()
    K1(T1(_)) as a => println(a)
  }
}

///|
fn main {
  let s = K1(T1(12, 34))
  s.f()
}
