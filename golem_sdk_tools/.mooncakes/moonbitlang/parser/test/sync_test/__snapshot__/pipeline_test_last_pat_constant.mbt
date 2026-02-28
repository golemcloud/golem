fn f(x : Int?) -> Int {
  match x {
    Some(v) => v
    None => 0
  }
}

fn init {
  println(f(None))
}
