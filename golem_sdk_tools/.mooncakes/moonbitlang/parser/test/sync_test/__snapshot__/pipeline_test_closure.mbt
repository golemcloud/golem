// normal closure
fn f() -> (() -> Double) {
  let x = 20.23
  let y = "Monday"
  let z = 9.11
  fn f1() {
    println(x)
    println(y)
    println(z)

    x - z
  }
  f1
}

fn init {
  println("\n==normal closure==\n")
  let f1 = f()
  println(f1())
}

// well-known closure with more than one free variables
fn g() -> Unit {
  let x = 20.23
  let y = "Monday"
  let z = 9.11
  fn g1() {
    println(x)
    println(y)
    println(z)

  }
  g1()

}
fn init {
  println("\n==well-known closure==\n")
  g()
}

// well-known closure with single variables
fn h() -> Unit {
  let x = 20.23
  fn h1() {
    println(x)
  }
  h1()

}
fn init {
  println("\n==well-known closure single capture==\n")
  h()
}
