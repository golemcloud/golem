
fn init {
  let z = 0
  fn id(x) {
    x
  }

  letrec even = fn (x) {
    if x == z { true } else { odd(x - 1) }
  }

  and odd = fn (x) {
    if x == 0 { false } else { even(x - 1) }
  }

  let _ = id(even) // make sure even and odd won't be optimized
  let _ = id(odd)
  println(if even(6) { 1 } else { 0 })
}

fn init {
  let z = 0
  letrec even = fn(x, b) {
    if x == z { b } else { odd(x - 1, 0L) }
  }

  and odd = fn(x, y: Int64) {
    if x == 0 { false } else { even(x - 1 - y.to_int(), true) }
  }

  println(if even(6, true) { 1 } else { 0 })
}
