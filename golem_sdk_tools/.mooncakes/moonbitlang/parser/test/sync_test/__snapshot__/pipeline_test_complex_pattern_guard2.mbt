///|
enum E {
  A(Bool)
  B(Bool)
}

///|
fn bug(b : Bool, f : (Bool) -> E) -> Int {
  match b {
    true if f(true) is (A(a) | B(a)) && f(a) is B(b) => {
      println(a)
      println(b)
      42
    }
    _ => 0
  }
}

///|
fn main {
  println(bug(true, fn(b) { if b { A(!b) } else { B(b) } }))
}
