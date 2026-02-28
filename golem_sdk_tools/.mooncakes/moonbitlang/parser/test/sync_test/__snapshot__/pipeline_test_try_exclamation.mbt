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

fn h() -> Unit {
  let a = try! {
    f(42) + f(42) + f(42)
  }
  println(a)
  
}

///|
fn main {
  println(try! f(42))
  try! println(f(42))
  println(g(_ => 42))
  h()
}
