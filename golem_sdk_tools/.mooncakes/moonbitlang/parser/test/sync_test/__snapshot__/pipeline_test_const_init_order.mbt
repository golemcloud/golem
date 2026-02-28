///|
struct T {
  x : Int
  y : Int?
} derive(Show)

///|
pub let a : T = { x: 0, y: Some(1) }

///|
fn main {
  println(a)
}
