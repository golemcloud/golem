//!build: -w -declaration_unimplemented

///|
struct S(Int)

///|
declare impl Show for S

///|
pub fn f(x : S) -> Unit {
  println(x)
}

///|
fn main {
  let a = S(10)
  println(42)
}
