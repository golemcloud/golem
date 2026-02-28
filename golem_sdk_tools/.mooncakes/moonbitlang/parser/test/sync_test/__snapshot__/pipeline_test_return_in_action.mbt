
// fn ignore(x: Int) {}

fn f(x: Bool) -> Int {
  ignore (match x {
    true => { return 0 }
    false => 1
  })
  2
}

fn init {
  println(f(true))
  println(f(false))
}
