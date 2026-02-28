
fn init {
  fn id(x) {
    x
  }

  letrec even = fn(x) {
    if x == 0 { true } else { odd(x - 1) }
  }

  and odd = fn(x) {
    if x == 0 { false } else { even(x - 1) }
  }

  let _ = id(even) // make sure even and odd won't be optimized
  let _ = id(odd)
  println(if even(6) { 1 } else { 0 })
}

