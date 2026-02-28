fn ignore(a: Int) {
  return
}

fn k(a: Int, b: Int) {
  return a
}

fn xtest_eq(a: Int, b: Int) -> Bool {
  if a == b { return true } else { return false }
}

fn ret_in_while() -> Int {
  while true {
    return 0
  }
}

fn init {
  return
}

let x = { return }

fn init {
  let mut x = 1
  fn foo(x) {
    if x > 2 { return 2 } else if x < -2 { return -2 } else { return x }
  }

  x = foo(10)
}

