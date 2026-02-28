suberror DemoError {
  InvalidChar(Pos, Char)
}

struct Pos {
  x : Int
}

fn foo() -> Int raise {
  raise DemoError::InvalidChar({ x: 1 }, 'a')
}

fn init {
  try foo() catch {
    DemoError::InvalidChar(pos, ch) =>
      println("invalid char at \{pos.x}: \{ch}")
    _ => println("unknown error")
  } else {
    _ => println("ok")
  }
}


suberror MultiError {
  E(Int, Double)
}

fn bar() -> Unit raise MultiError {
  raise E(42, 4.2)
}

fn init {
  try bar() catch {
    E(i, d) => println("E(\{i}, \{d})")
  }
}
