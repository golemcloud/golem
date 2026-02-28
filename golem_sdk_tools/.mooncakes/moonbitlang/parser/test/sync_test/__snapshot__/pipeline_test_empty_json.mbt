///|
fn main {
  guard  ({} : Json) is { .. } 
  f({})
  f({ "x": 100, "y": 200 })
  f({ "y": 200 })
  f([])
}

///|
fn f(j : Json) -> Unit {
  match j {
    { .. } => println("empty")
    { "x": 100, .. } => println("x is 100")
    _ => println("other")
  }
}
