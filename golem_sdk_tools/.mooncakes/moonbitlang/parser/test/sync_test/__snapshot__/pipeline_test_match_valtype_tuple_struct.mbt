///|
#valtype
struct T(Int,Int)

///|
fn f(t : T) -> Int {
  let T(a, b) = t
  a + b
}

///|
fn main {
  println(f(T(1, 2)))
}
