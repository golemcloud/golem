///|
fn f(x : String, b : Bool) -> Unit {
  match (x, b) {
    (['a', .. rest], true) => println(rest)
    ([.. rest, _], _) => println(rest)
    _ => ()
  }
}

///|
fn main {
  f("abc", false)
}
