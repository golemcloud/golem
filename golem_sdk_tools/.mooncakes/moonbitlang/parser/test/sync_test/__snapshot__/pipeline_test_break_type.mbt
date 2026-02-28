fn f(x: Int) -> Unit {
  let mut a = Option::Some(x)
  while true {
    let b = match a {
      None => break
      Some(i) => Option::Some(i)
    }
    println(b)
    a = None
  }
}

fn g(x: Int) -> Unit {
  let mut a = Option::Some(x)
  while true {
    let b = match a {
      None => continue
      Some(i) => Option::Some(i)
    }
    println(b)
    break
  }
}

fn init {
  f(42)
  g(42)
}
