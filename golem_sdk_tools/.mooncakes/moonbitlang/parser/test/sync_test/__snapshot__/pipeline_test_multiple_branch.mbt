fn test1(b: Bool) -> Unit {
  let (a, b) = if b { (1, 2) } else { (3, 4) }
  println(a)
  println(b)
}

fn init {
  test1(true)
  test1(false)
}

