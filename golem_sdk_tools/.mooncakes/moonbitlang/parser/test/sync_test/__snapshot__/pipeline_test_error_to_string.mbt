suberror Error1 {
  Error1(String)
}

suberror Error2 {
  Error2(String)
} derive(Show)

priv suberror Error3 {
  Error3(Int)
} derive(Show)

suberror Error4 {
  A
  B
}

fn error_to_string(e: Error) -> String = "%error.to_string"

fn f1() -> Unit raise Error {
  raise Error1("1")
}

fn f2() -> Unit raise Error {
  raise Error2("2")
}

fn f3() -> Unit raise Error {
  raise Error3(42)
}

fn f4() -> Unit raise Error {
  raise A
}

impl Show for Error4 with output(self, logger) {
  match self {
    A => logger.write_string("AAA")
    B => logger.write_string("BBB")
  }
}


fn main {
  try f1() catch { e => println(error_to_string(e)) }
  try f2() catch { e => println(error_to_string(e)) }
  try f3() catch { e => println(error_to_string(e)) }
  try fail("xxx") catch { e => println(error_to_string(e)) }
  try f4() catch { e => println(error_to_string(e)) }
}
