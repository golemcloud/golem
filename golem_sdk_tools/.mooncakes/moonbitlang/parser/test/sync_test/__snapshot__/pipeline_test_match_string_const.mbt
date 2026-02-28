///|
const False : String = "😭False"

///|
const True : String = "true😭"

///|
fn f(s : String) -> Unit {
  match s {
    [.. False, .. rest, .. True ] => println(rest)
    _ => println(s)
  }
}

///|
fn main {
  f("false")
  f([..False, '😭', ..True])
}
