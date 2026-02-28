///|
#cfg(false)
fn init {
  println("init with cfg(false)")
}

///|
#cfg(true)
fn init {
  println("init with cfg(true)")
}

///|
#cfg(false)
fn main {
  println("main with cfg(false)")
}

///|
#cfg(true)
fn main {
  println("main with cfg(true)")
}

///|
#cfg(false)
test {
  println("test with cfg(false)")
}

///|
#cfg(true)
test {
  println("test with cfg(true)")
}
