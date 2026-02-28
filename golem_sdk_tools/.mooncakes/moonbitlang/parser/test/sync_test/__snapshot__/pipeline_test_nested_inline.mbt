fn maybe(x : Int) -> Option[String] {
  if x == 42 { Some("answer") } else { None }
}

fn init {
  fn f(x : Int) {
    match maybe(x) {
      Some(ans) => return ans
      None => ()
    }
    panic()
  }
  println(f(42))
}
