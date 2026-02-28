///|
#inline(never)
fn f1(x : Int) -> Int {
  x + 1
}

///|
#inline
pub fn f2(x : Int) -> Int {
  x + x + x + x + x + x + x
}

///|
fn main {
  println(f1(42))
  println(f2(42))
}
