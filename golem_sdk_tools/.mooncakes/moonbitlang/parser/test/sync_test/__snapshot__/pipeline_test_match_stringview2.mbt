///|
pub fn f(x : StringView) -> Unit {
  match x {
    ['h', 'i', .. rest] => println(rest)
    ['i', 'f'] => println("if")
    _ => println("else")
  }
}

///|
fn main {
  f("hi"[:])
  f("if".view())
}
