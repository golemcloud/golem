struct T {
  x: Int
  f: (Int) -> Unit
}

fn make_t(x: Int, s1: String, s2: String) -> T {
  letrec even= fn (a: Int) -> Unit {
    if a == 0 {
      println(s2)
    } else {
      odd(a-1)
    }
  }
  and odd =fn(a: Int) -> Unit {
    if a == 0 {
      println(s1)
    } else {
      even(a-1)
    }
  }
  T::{x, f: odd}
}

fn init {
  let t = make_t(42, "even", "odd")
  (t.f)(t.x)
}
