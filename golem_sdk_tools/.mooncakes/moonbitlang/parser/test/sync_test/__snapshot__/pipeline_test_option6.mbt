fn f(x: Option[Int], y: Option[Int]) -> Bool {
  match (x, y) {
    ((Some(_)| None), None) => true
    (Some(i), Some(j)) => i < j
    (None, Some(_)) => false
  }
}

fn init {
  println(f(Some(1), None))
  println(f(Some(1), Some(2)))
  println(f(None, Some(2)))
  println(f(None, None))
}
