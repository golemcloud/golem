///|
let c : (Int, Int) = (1, 2)

///|
struct S {
  s1 : String
  s2 : Int64
} derive(Show)

///|
let s : S = { s1: "hello", s2: 42 }

///|
enum E {
  A
  B(Int, String)
} derive(Show)

///|
let e1 : E = E::A

///|
let e2 : E = E::B(1, "hello")

///|
let a : FixedArray[Double] = [1.2, 2.3, 3.4, 4.5, 5.6, 6.7, 7.8, 8.9]

///|
let b : FixedArray[String] = [
  "hello", "world", "foo", "bar", "baz", "qux", "quux", "corge",
]

///|
fn main {
  println(c)
  println(s)
  println(e1)
  println(e2)
  println(a)
  println(b)
  b[0] = String::make(3, 'a')
  b[0] = String::make(4, 'a')
  println(b)
}
