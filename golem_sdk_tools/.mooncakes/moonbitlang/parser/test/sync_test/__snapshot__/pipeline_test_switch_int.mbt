fn test1(x: Int, y: Int) -> Unit {
  if (x == y) { println(".") } else { println("x") }
}

fn f(x: Int) -> Int {
  match x {
    0 => 1
    1 => 2
    2 => 3
    3 => 4
    _ => 5
  }
}

fn g(x: Int) -> Int {
  match x {
    0 => 1
    2 => 3
    3 => 4
    1 => 2
    _ => 5
  }
}

// will not use [br_table]
fn h(x: Int) -> Int {
  match x {
    0 => 1
    2 => 3
    3 => 4
    1 => 2
    7 => 100
    _ => 5
  }
}

// use [br_table]
fn i(x : Int) -> Int {
  match x {
    42 => 0
    45 => 3
    43 => 1
    44 => 2
    _ => 42
  }
}


fn init {
  test1(f(0), 1)
  test1(f(1), 2)
  test1(f(2), 3)
  test1(f(3), 4)
  test1(f(4), 5)
  test1(f(5), 5)
  test1(f(-1), 5)
  test1(g(0), 1)
  test1(g(1), 2)
  test1(g(2), 3)
  test1(g(3), 4)
  test1(g(4), 5)
  test1(g(5), 5)
  test1(g(-1), 5)
  test1(h(0), 1)
  test1(h(1), 2)
  test1(h(2), 3)
  test1(h(3), 4)
  test1(h(4), 5)
  test1(h(5), 5)
  test1(h(-1), 5)
  test1(h(7), 100)
  test1(i(42), 0)
  test1(i(43), 1)
  test1(i(44), 2)
  test1(i(45), 3)
  test1(i(0), 42)
}
