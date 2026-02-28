suberror MyErr derive(Show)

fn f(coin : Bool) -> Int raise MyErr {
  if coin {
    42
  } else {
    raise MyErr
  }
}

fn main {
  println(try? f(true))
  println(try? f(false))
  println(try? f(f(true) < 0))
  fn outside(coin) -> _ raise MyErr {
    let x = f(coin)
    try? f(x < 0)
  }
  println(try? outside(true))
  println(try? outside(false))
}
