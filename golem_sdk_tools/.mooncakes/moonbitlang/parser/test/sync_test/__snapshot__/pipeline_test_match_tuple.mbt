
fn init {
  println(
    match (1, 2) {
      (x, y) => x + y
    }
  )
  println(
    match (Option::Some(1), Option::None) {
      (Some(x), Some(y)) => x + y
      (Some(x), None) | (None, Some(x)) => x
      (None, None) => 0
    }
  )
  println(
    match (1, 2) {
      (0, x) => x
      (y, 0) => y
      t => t.0 + t.1
    }
  )
  let t = (3, 4)
  let (x, _) = t
  println(x)
}
