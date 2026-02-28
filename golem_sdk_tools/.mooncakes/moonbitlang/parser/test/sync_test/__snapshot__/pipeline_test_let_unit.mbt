fn f(coin : Bool) -> Unit {
  let x = if coin {
    println("true")
  }
  ignore(x)
}

fn main {
  f(true)
}
