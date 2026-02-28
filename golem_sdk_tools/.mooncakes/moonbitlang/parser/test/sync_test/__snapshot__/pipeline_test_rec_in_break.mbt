fn main {
  fn rec_in_break(n : Int) {
    for i = 0; i < 3; i = i + 1 {
      if i > n {
        break rec_in_break(n + 1)
      }
    } else {
      42
    }
  }
  println(rec_in_break(1))
}
