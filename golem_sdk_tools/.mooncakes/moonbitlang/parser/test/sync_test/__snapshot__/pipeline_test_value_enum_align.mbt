///|
#valtype
enum E {
  A(Byte)
  B(Byte)
} derive(Show)

///|
fn f(e : E) -> E {
  match e {
    A(b) => B(b + 1)
    B(b) => A(b - 1)
  }
}

///|
fn main {
  let e1 = A(1)
  let e2 = B(2)
  println(f(e1))
  println(f(e2))
}
