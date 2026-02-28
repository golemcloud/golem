///|
fn[A : Show, B : Show] f(a : A, b : B) -> String {
  struct B {
    f1 : Array[A]
    f2 : String
  } derive(Show)
  let b1 = { f1: [a], f2: "b1" }
  "\{a} \{b} \{b1}"
}

///|
fn main {
  println(f("xxx", 42))
}
