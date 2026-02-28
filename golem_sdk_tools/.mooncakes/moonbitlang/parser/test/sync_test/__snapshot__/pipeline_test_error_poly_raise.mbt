///|
suberror Err {
  Err1
  Err2
} derive(Show)

///|
fn error_poly_but_may_raise(flag : Bool, f : () -> Unit raise?) -> Unit raise {
  if flag {
    f()
  } else {
    raise Err1
  }
}

///|
fn init {
  println(try? error_poly_but_may_raise(true, () => println("callback")))
  println(try? error_poly_but_may_raise(false, () => println("callback")))
  println(try? error_poly_but_may_raise(true, () => raise Err2))
  println(try? error_poly_but_may_raise(false, () => raise Err2))
}
