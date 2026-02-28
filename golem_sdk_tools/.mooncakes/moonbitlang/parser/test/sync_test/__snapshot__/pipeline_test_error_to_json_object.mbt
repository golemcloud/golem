///|
fn f(x : &ToJson) -> Unit {
  println(x.to_json())
}

///|
suberror E {
  E(Int)
}

///|
fn main {
  f((E(42) : Error))
}
