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
    b: {
      println("b")
      42
    },
    a: {
      println("a")
      24
    },
    c: 0,
    d: 0,
  }
  let _ = S::{
    ..{
      println(0)
      a
    },
    b: {
      println("b")
      42
    },
    a: {
      println("a")
      24
    },
  }

}
