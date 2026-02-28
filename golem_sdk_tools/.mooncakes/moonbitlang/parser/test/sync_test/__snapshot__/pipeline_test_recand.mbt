fn init {
  letrec even = fn (x: Int) {
    if x == 0 {
      true
    } else {
      odd(x - 1)
    }
  }
  and odd = fn (x: Int) {
    if x == 0 {
      false
    } else {
      even(x - 1)
    }
  }
  println(even(10))
}

fn init {
  letrec even: (Int) -> Bool = x =>
    if x == 0 {
      true
    } else {
      odd((x - 1).to_int64())
    }
  and odd: (Int64) -> Bool = x =>
    if x == 0 {
      false
    } else {
      even((x - 1).to_int())
    }
  println(even(10))
}
