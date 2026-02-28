suberror XXError {
  XXError(Int)
}

pub fn f() -> Unit raise {
  raise fail("xxx")
}

pub fn g() -> Unit {
  try {
    f()
  } catch {
    XXError(_) as c => ignore(c)
    _ => ()
  }
}

pub fn h() -> Unit {
  try {
    f()
  } catch {
    XXError(_) as c => ignore(c)
    _ => ()
  }
}

fn main {
  g()
  h()
}
