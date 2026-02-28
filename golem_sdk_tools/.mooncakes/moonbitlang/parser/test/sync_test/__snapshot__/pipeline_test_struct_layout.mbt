///|
struct S {
  y : Int16
  mut x : Byte
  z : Int
} derive(Show)

///|
fn f(s : S) -> Unit {
  println(s.x)
  println(s.y)
  println(s.z)
  s.x = 10
}

///|
fn main {
  let s = S::{ x: 1, y: 2, z: 3 }
  f(s)
  println(s)
}
