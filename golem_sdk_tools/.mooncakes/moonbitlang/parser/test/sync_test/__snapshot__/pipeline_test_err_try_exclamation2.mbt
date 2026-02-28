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

fn g(f : (Int) -> Int raise? ) -> Int {
  try! f(42)
}

///|
fn main {
  println(g(_ => raise E(42)))
}
