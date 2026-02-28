fn f() -> Int {
  42
}

fn test_show(x: &Show) -> Unit {
  println(x.to_string())
}

fn init {
  test_show(f() as &Show)
}
