///|
fn f(xs : @list.List[Int]) -> Unit {
  l~: for i in 0..=1 {
    for j in xs {
      if j == 0 {
        break l~
      }
    }
  } else {
    println("else")
  }
}

///|
fn init {
  f(@list.from_array([1, 2, 3]))
  f(@list.from_array([0]))
}
