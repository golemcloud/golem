///|
suberror Error1 {
  Error1(String)
}

///|
suberror Error2 {
  Error2(String)
} derive(Debug)

///|
priv suberror Error3 {
  Error3(Int)
} derive(Debug)

///|
suberror Error4 {
  A
  B
}

///|
fn error_to_repr(e : Error) -> Repr = "%error.to_repr"

///|
fn f1() -> Unit raise Error {
  raise Error1("1")
}

///|
fn f2() -> Unit raise Error {
  raise Error2("2")
}

///|
fn f3() -> Unit raise Error {
  raise Error3(42)
}

///|
fn f4() -> Unit raise Error {
  raise A
}

///|
impl Debug for Error4 with to_repr(self) {
  match self {
    A => Repr::ctor("AAA", [])
    B => Repr::ctor("BBB", [])
  }
}

///|
fn main {
  f1() catch {
    e => println(@debug.render(error_to_repr(e)))
  }
  f2() catch {
    e => println(@debug.render(error_to_repr(e)))
  }
  f3() catch {
    e => println(@debug.render(error_to_repr(e)))
  }
  fail("xxx") catch {
    e => println(@debug.render(error_to_repr(e)))
  }
  f4() catch {
    e => println(@debug.render(error_to_repr(e)))
  }
}
