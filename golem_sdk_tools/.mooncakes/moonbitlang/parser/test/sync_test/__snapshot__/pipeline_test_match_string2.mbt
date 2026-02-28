///|
fn f(s : String) -> Unit {
  match s {
    [a, b, c, ..] => {
      println(a)
      println(b)
      println(c)
    }
    _ => ()
  }
}

///|
fn main {
  f("abcdef")
}
