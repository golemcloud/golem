enum T {
  A(Int)
  B
  C
}

pub fn as_null(x : T) -> Unit? {
  match x {
    B => Some(())
    _ => None
  }
}

fn init {
  let y = as_null(C)
  println(y)
}
