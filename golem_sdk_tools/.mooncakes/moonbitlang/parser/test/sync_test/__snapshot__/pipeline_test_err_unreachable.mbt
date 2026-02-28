fn[T] unreachable_() -> T = "%unreachable"

fn[A] get_exn(x: A?) -> A {
  match x {
    Some(a) => a
    None => unreachable_()
  }
}

fn init {
  let _ = get_exn(None)
}
