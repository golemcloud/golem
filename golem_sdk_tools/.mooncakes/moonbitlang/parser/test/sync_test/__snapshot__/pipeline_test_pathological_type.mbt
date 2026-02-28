struct T {
  v: T
}

pub fn tt(x: T) -> T {
  match x {
    { v } => v
  }
}
