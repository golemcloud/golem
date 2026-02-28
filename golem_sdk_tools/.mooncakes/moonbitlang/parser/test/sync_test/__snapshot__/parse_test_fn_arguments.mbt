// Parse different kinds of function arguments.

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

// Local functions 
fn locals() -> Unit {
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

  // error: labelled arguments are not allowed in local functions
  fn labelled(a~ : Int, b~ : Int) -> Int {
    a + b
  }

  // error: labelled arguments are not allowed in local functions
  fn question(a? : Int, b? : Int) -> Int? {
    a.bind(fn(a) { b.bind(fn(b) { Some(a + b) }) })
  }

  // error: labelled arguments are not allowed in local functions
  fn optional(a~ : Int = 1, b~ : Int = 2) -> Int {
    a + b
  }
}

// Lambda expressions
fn lambdas() {
  let positional = fn(a, b) { a + b }
  let positional2 = fn(a : Int, b: Int) { a + b }
  let discard_positional = fn(_: Int, _: Int) { 1 }
  let discard_positional2 = fn(_, _) { 1 }

  // error: labelled arguments are not allowed in lambda expressions
  let labelled = fn(a~ : Int, b~ : Int) { a + b }
  let question = fn(a? : Int, b? : Int) { a.bind(fn(a) { b.bind(fn(b) { Some(a + b) }) }) }
  let optional = fn(a~ : Int = 1, b~ : Int = 2) { a + b }
}
