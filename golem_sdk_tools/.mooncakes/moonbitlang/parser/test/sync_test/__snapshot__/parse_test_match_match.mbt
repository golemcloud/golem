fn f(x: Int) -> Int {
  match if x is 42 { () } {
    _ => 0
  }
}

fn g(x: Int) -> Int {
  if match x { _ => true } {
    42
  } else {
    0
  }
}
