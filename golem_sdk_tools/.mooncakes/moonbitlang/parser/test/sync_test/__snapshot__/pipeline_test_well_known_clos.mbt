fn f(x: Int, y: Int, z: Int) -> Unit {
  fn addxy() {
    x+y
  }
  fn addyz() {
    y+z
  }
  println(addxy() + addyz())
}

fn init {
  f(1, 2, 3)
}
