///|
#valtype
struct E {
  a : Int
  b : Int64
  c : Double
} derive(Show)

///|
enum F {
  A(E, String)
  B(Bytes, E)
} derive(Show)

///|
suberror X {
  X(Int)
} derive(Show)

///|
fn f(a : Int, b : Int64, c : Double) -> F raise X {
  if a > 0 {
    F::A(E::{ a, b, c }, "string")
  } else if a < 0 {
    F::B("bytes", E::{ a, b, c })
  } else {
    raise X(a)
  }
}

///|
fn main {
  println(try? f(1, 2, 3.14))
  println(try? f(-1, 2, 3.14))
  println(try? f(0, 2, 3.14))
}
