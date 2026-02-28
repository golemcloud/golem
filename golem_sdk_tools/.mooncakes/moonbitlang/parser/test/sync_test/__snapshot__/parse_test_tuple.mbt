
fn init {
  let x = (1, 2, (3, (4, 5)))
  println(x.0)
  println(x.1)
  println(x.2.0)
  println(x.2.1.0)
  println(x.2.1.1)
}
