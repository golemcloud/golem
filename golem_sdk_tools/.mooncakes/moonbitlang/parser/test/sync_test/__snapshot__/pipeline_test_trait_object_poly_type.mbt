fn f(x: &Show) -> Unit {
  println(x)
}

fn[X: Show] poly(x: X) -> Unit {
  f(Option::Some(x))
}

fn init {
  f(Option::Some(42))
  f(Option::Some("xy"))
  poly(42)
  poly("xy")
}
