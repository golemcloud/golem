fn init {
  println(
    match (Option::Some(1), Option::None) {
      (Some(x), None) | (None, Some(x)) => x
      _ => 42
    }
  )
}
