///|
pub(all) struct T {
  a : Int
}

///|
declare pub fn T::f2(x : Self) -> Int
///|
pub fn T::f2(x : Self) -> Int {
  x.a + 1
}

///|
fn main {
  let t = T::{ a: 10 }
  println(t.f2())
}
