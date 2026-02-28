pub fn test1(b:Bool) -> Unit {
  let (x, y) = if b {
    (1, 2)
  } else {
    (3, 4)
  }
  println(x)
  println(y)
}

