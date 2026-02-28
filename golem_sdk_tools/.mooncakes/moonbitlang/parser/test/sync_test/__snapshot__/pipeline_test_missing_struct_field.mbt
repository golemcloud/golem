//!build: -w -56

///|
struct S {
  a : Int
  b : Double
}

///|
fn f1(x : S) -> Unit {
  match x {
    { a: i } => println(i)
  }
}

///|
fn f2(x : S) -> Unit {
  match x {
    { b: f, .. } => println(f)
  }
}

///|
fn f3(x : S) -> Unit {
  match x {
    { } => println("xx")
  }
}

///|
fn main {
  f1({ a: 1, b: 1.0 })
  f2({ a: 1, b: 1.0 })
  f3({ a: 1, b: 1.0 })
}
