//!build: -w -28

pub fn f(x : Int) -> Unit {
  match x {
    1 => ()
    2 => ()
    3 => ()
    4..=10 =>  ()
    11 => ()
    12 => ()
    13 => ()
    _ => ...
  }
}

fn main {
  println(f(1))
}
