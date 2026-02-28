//!build: -w -57

enum H {
  A(mut i~ : Int )
  B
} derive(Show)

pub fn hello(x : H) -> Unit {
  if x is (A as y) {
    y.i = 200
  }
}

fn main {
  let v = A(i=2)
  hello(v)
  println(v)
}