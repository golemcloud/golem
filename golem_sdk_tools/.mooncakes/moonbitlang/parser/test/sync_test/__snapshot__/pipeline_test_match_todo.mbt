//!build: -w -a
fn f(x:Int) -> Unit {
  match x {
    0 => println(0)
    ...
  }
}

fn main {
  f(0)
}
