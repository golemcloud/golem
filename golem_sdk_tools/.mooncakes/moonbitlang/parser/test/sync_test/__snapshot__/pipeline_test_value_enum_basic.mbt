///|
enum F {
  A(E)
  B(E, String)
} derive(Show)

///|
#valtype
enum E {
  A(Int)
  B(Byte, Int)
  C(Double, Int64, Bool)
} derive(Show)

///|
struct B[T] {
  e1 : T
  i : Int
  s : String
  e2 : E
} derive(Show)

///|
fn test_array() -> Unit {
  let arr = []
  arr.push(E::B(2, 3))
  for i in 0..=10 {
    arr.push(E::A(i))
  }
  println(arr)
}

///|
fn test_f(f : F) -> Unit {
  println(f)
  ignore(f)
}

///|
fn main {
  println(E::A(1))
  println(E::B(2, 3))
  println(E::C(2.0, 333, true))
  test_array()
  test_f(F::A(E::A(1)))
  test_f(F::B(E::B(2, 3), (4).to_string()))
  println(B::{ e1: E::A(1), i: 2, s: (3).to_string(), e2: E::B(4, 5) })
}
