///|
enum E {
  A
  B
}

///|
fn bug(b : Bool, f : (Bool) -> E) -> Int {
  match b {
    true if f(true) is (A | B) && f(false) is B => 1
    _ => 0
  }
}

///|
fn main {
  println(bug(true, fn(b) { if b { A } else { B } }))
}
