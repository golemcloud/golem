///|
struct S {
  x : Int
  y : Int
}

///|
fn g1(s : S) -> Unit {
  match s {
    { x: 0, y } => println(y)
    { x, y: _ } => g1({ ..{ x, y: s.y + 1 }, x: x - 1 })
  }
}

///|
fn main {
  g1(S::{ x: 2, y: 0 })
}
