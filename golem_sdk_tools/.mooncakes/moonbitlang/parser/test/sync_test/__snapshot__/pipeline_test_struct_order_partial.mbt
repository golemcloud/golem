///|
pub struct S {
  a : Int
  b : Int
  c : Int
  d : Int
}

///|
fn main {
  let a = S::{
    a: {
      println("a")
      24
    },
    b: {
      println("b")
      42
    },
    d: {
      println("d")
      0
    },
    c: {
      println("c")
      0
    },
  }
  let _ = S::{
    ..{
      println(0)
      a
    },
    a: {
      println("a")
      24
    },
    c: {
      println("c")
      42
    },
    b: {
      println("b")
      42
    },
  }

}
