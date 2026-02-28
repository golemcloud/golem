///| only loopify
fn loop1() -> Int {
  fn f(x : Int) {
    if x < 0 {
      return x
    }
    if x > 0 {
      return f(x - 1)
    }
    42
  }

  f(10) + f(-1)
}

///| loopify and contify
fn loop2(b : Bool) -> Int {
  fn f(x : Int) {
    if x < 0 {
      return x
    }
    if x > 0 {
      return f(x - 1)
    }
    42
  }

  if b {
    f(10)
  } else {
    f(-1)
  }
}

///| toplevel function loopify
fn loop3(x : Int) -> Int {
  if x < 0 {
    return x
  }
  if x > 0 {
    return loop3(x - 1)
  }
  loop3(-1) + 42
}

///| return with complex expr
fn loop4(b : Bool) -> Int {
  fn f(x : Int) {
    if x < 0 {
      return x
    }
    if x > 0 {
      return if b { f(x - 1) } else { 100 }
    }
    42
  }

  f(10) + f(-1)
}

///|

///|
fn main {
  println(loop1())
  println(loop2(true))
  println(loop2(false))
  println(loop3(10))
  println(loop4(true))
  println(loop4(false))
}
