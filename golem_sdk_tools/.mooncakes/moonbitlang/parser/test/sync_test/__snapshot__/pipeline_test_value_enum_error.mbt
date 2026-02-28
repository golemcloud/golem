///|
#valtype
enum E {
  A
  B(Int)
  C(Double, Double)
} derive(Show)

///|
suberror X {
  X(Int)
} derive(Show)

///|
fn f(i : Int) -> E raise X {
  if i < 10 {
    A
  } else if i < 20 {
    B(i)
  } else if i < 30 {
    C(i.to_double(), i.to_double())
  } else {
    raise X(i)
  }
}

///|
fn main {
  println(try? f(5))
  println(try? f(15))
  println(try? f(25))
  println(try? f(35))
}
