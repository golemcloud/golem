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

fn h() -> Unit {
  let a = try! {
    f(42) + f(42) + f(-1)
  }
  println(a)
  
}

///|
fn main {
  h()
}
