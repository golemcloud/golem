fn bad(capture : Double) -> Unit {
  letrec f1 = fn(i) {
    if i == 0 {
      println(capture)
    } else {
      f2(i - 1)
    }
  }
  and f2 = fn(i) {
    f1(i)
  }
  // make sure f1 is compiled to a closure
  id(f1)(10)
}

fn init {
  bad(42.0)
}

fn[X] id(x : X) -> X { x }
