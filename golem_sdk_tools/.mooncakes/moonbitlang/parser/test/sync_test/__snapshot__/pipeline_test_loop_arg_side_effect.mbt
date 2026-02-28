fn f(x: Int) -> Unit {
  loop ({println(1); x}, {println(2); x}) {
    (a, b) if a != b => continue (a + 1, b)
    _ => return
  }
}

fn main {
  f(42)
}
