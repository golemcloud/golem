///|
#valtype
struct S {
  a : Int
  b : Bytes
  c : Int64?
  d : Bool
} derive(Show)

///|
enum E {
  A(mut a~ : S, Int)
  B(FixedArray[Int], S)
} derive(Show)

///|
fn make_s(i : Int) -> S {
  S::{
    a: i,
    b: Bytes::makei(i, i => i.to_byte()),
    c: Some(i.to_int64()),
    d: true,
  }
}

///|
fn f1(e : E) -> E {
  match e {
    A(_) as x => {
      let res = B([1, 2, 3], x.a)
      x.a = make_s(3)
      res
    }
    B(_) as y => y
  }
}

///|
fn main {
  let e1 = E::A(a=make_s(1), 2)
  println(e1)
  let e2 = f1(e1)
  println(e1)
  println(e2)
}
