enum S[T] {
  S0
  S1
  S2
  S3
  S4
}

// CR: poly functiion will not be exported
// we should add a warning if so
pub fn[T] f1(x: S[T]) -> Int {
  match x {
    S0 => 0
    S1 => 1
    S2 => 2
    S3 => 3
    S4 => 4
  }
}

fn init {
  println(f1(S0))
  println(f1(S1))
  println(f1(S2))
  println(f1(S3))
  println(f1(S4))
}