///|
fn positional(a : Int, b : Int) -> Int {
  a + b
}

fn discard_positional(_: Int, _: Int) -> Int {
  1
}

///|
fn labelled(a~ : Int, b~ : Int) -> Int {
  a + b
}

///|
fn question(a? : Int, b? : Int) -> Int? {
  a.bind(fn(a) { b.bind(fn(b) { Some(a + b) }) })
}

///|
fn optional(a~ : Int = 1, b~ : Int = 2) -> Int {
  a + b
}

///|
fn init {
  let x1 = positional(1, 2)
  println(x1) // 3
  let x2 = discard_positional(1, 2)
  println(x2) // 1
  let x3 = labelled(a=1, b=2)
  println(x3) // 3
  let x4 = question(a=1, b=2)
  println(x4) // Some(3)
  let x5 = optional()
  println(x5) // 3
}

// Local functions
fn init {
  fn positional(a : Int, b : Int) -> Int {
    a + b
  }

  fn positional2(a, b) {
    a + b
  }

  fn discard_positional(_: Int, _: Int) -> Int {
    1
  }

  fn discard_positional2(_, _) {
    1
  }

  let x1 = positional(1, 2)
  println(x1) // 3
  let x2 = positional2(1, 2)
  println(x2) // 3
  let x3 = discard_positional(1, 2)
  println(x3) // 1
  let x4 = discard_positional2(1, 2)
  println(x4) // 1
}

// Lambda expressions
fn init {
  let positional = fn(a, b) { a + b }
  let x1 = positional(1, 2)
  println(x1) // 3

  let discard_positional = fn(_, _) { 1 }
  let x2 = discard_positional(1, 2)
  println(x2) // 1
}
