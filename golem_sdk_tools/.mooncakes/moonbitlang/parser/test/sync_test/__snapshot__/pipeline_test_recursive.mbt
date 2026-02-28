fn hello(x: Int) -> Int {
  x
}

fn even(x: Int) -> Bool {
  match x {
    0 => true
    1 => false
    _ => odd(x - 1)
  }
}

fn add(x: Int, y: Int) -> Int {
  x + y + 100
}

fn odd(x: Int) -> Bool {
  match x {
    0 => false
    1 => true
    _ => even(x - 1)
  }
}

