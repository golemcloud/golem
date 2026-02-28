fn init {
  let one = 1
  letrec a = fn() {
    b()
  }

  and b = fn() {
    c()
  }

  and c = fn() {
    one
  }

  println(a())
}

fn init {
  let one = 1.1
  letrec a = fn() {
    b()
  }

  and b = fn() {
    c()
  }

  and c = fn() {
    one
  }

  println(a())
}

fn init {
  let one = 1.1
  letrec b = fn() {
    c()
  }

  and c = fn() {
    one
  }

  let d = b

  fn a() {
    d()
  }


  println(a())
}
