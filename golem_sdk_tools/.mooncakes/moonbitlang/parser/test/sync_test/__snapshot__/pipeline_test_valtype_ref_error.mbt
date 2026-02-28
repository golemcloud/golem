///|
#valtype
struct S {
  a : Int
  b : Bytes
  c : Int64?
  d : Bool
} derive(Show)

///|
suberror E {
  E(Int)
} derive(Show)

///|
fn make_s(i : Int) -> S raise E {
  if i < 0 {
    raise E(i)
  } else {
    S::{
      a: i,
      b: Bytes::makei(i, i => i.to_byte()),
      c: Some(i.to_int64()),
      d: true,
    }
  }
}

///|
fn main {
  println(try? make_s(-1))
  println(try? make_s(5))
}
