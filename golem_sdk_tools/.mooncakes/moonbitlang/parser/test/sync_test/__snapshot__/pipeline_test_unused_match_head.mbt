
fn f() -> Int {
  println("f()\n")
  0
}

fn init {
  match f() {
    _ => println("done\n")
  }
}
