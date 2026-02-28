///|
struct S {
  mut a : &Show
  b : (Int, &Show)
} derive(Show)

///|
fn main {
  let s1 = S::{ a: 42, b: (42, (42, 42)) }
  println(s1)
  let bs = b"abcde"
  s1.a = bs[1:3]
  println(s1)
}
