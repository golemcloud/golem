fn f(x: String) -> Int {
  match x {
    "ok11" => 1
    "ok2" => 2
    "ok3" => 3
    _ => 4
  }
}
fn no_inline(x : String, y : Int) -> Int {
  println("noinline")
  f("ok \{x} \{y + 3}")
}


fn init {
  println(f("ok11"))
  println(f("ok2"))
  println(f("ok3"))
  println(f("ok33"))
  println(f("ok" + (3).to_string()))
  println(no_inline("ok", 3))
}

