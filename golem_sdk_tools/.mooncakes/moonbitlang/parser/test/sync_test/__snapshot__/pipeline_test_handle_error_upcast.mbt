struct T {}

fn T::new() -> T raise {
  if false {
    fail("Error")
  }
  T::{  }
}

pub(open) trait Any {}

struct Test {
  f : () -> &Any raise
}

fn main {
  let _ : Test = { f: fn() { T::new() } }

}
