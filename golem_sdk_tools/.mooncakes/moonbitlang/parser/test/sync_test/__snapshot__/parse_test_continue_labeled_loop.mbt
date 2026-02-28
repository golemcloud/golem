///|
pub fn f() -> Unit {
  loop () {
    _ =>
      continue l~: loop 0 {
          _ => ...
        }
  }
}

pub fn g() -> Unit {
  l~: loop () {
    _ =>
      continue l~ l~: loop 0 {
          _ => ...
        }
  }
}