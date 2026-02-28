//!build: -w -27

fn assirt(cond: Bool, msg: String, loc~ : SourceLoc = _) -> Result[Unit, String] {
  if not(cond) {
    Err("(\{loc}) assertion failed: \{msg}")
  } else {
    Ok(())
  }
}

fn[T: Eq + Show] assert_eq(lhs: T, rhs: T, loc~ : SourceLoc = _) -> Result[Unit, String] {
  assirt(lhs == rhs, "\{lhs} is not equal to \{rhs}", loc~)
}


fn f() -> Result[Unit, String] {
  assert_eq(1, 2)
}

fn init {
  println(f())
}
