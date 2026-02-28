///|
pub fn f(x : StringView) -> Unit {
  match x {
    [x1, x2, 'x', 'x', .. rest] => {
      println(x1)
      println(x2)
      println(rest)
    }
    ['a', 'b', 'c', 'd'] => println("abcd")
    _ => println("else")
  }
}

///|
fn main {
  f("abxxx".view())
  f("abcd".view())
  f("abx".view())
}
