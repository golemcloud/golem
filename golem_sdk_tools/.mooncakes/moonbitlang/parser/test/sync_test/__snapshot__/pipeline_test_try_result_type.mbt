fn f(c : Bool) ->  Int64 raise {
  if c {
    0L
  } else {
    fail("1")
  }
}

fn g(c : Bool) -> Unit raise {
  try {
    f(c)
  } catch {
    _ => ()
  } else {
    _ => fail("!")
  }
}

fn init {
  println(try? g(true))
  println(try? g(false))
}
