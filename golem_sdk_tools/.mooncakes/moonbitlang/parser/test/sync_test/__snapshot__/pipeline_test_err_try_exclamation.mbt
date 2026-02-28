///|
suberror E {
  E(Int)
}

///|
fn f(x : Int) -> Int raise E {
  if x < 0 {
    raise E(x)
  }
  x
}

///|
fn main {
  try! println(f(-1))
}
