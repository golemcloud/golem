fn f(x: Int) -> Unit {
  while true {
    match x {
      0 => break
      _ => println(2)
    }
    println(3)
    break
  }
  println(1)
}

fn init {
  f(0)
}
