///|
struct S {
  a : String
}

///|
fn main {
  let mut s = S::{ a: "x" }
  println(s.a)
  s = S::{ a: "xx" }
  println(s.a)
}
