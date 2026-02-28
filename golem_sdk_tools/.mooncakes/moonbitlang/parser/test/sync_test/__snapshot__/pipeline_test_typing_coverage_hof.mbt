fn[T] apply_twice(x : T, f : (T) -> T) -> T {
  f(f(x))
}

fn[A, B, C] compose(f : (B) -> C, g : (A) -> B) -> (A) -> C {
  fn (x : A) { f(g(x)) }
}

fn[A, B] map_opt(x : A?, f : (A) -> B) -> B? {
  match x {
    Some(v) => Some(f(v))
    None => None
  }
}

fn init {
  let inc = x => x + 1
  let double = x => x * 2
  let f = compose(inc, double)
  let _ = apply_twice(1, inc)
  let _ = f(3)
  let _ = map_opt(Some(5), inc)
  let _ = map_opt(None, inc)
  ()
}
