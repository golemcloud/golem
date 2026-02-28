enum T {
  A
  Bbb
}

fn init {
  let x : T = A
  let y = T::A
  let z = T::Bbb
  ()
}

fn hello() -> T {
  Bbb
}

fn init {
  let x = fn() { T::Bbb }
  ()
}

