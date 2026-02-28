///|
struct S(Int,Int)

///|
fn f(s : S) -> Int {
  match s {
    S(x, y) => x + y
  }
}

///|
fn main {
  println(f(S(1, 2)))
}
