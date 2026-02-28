///|
fn is_file(s : String) -> Bool raise Failure {
  if s == "file" {
    true
  } else {
    raise Failure("not a file")
  }
}

///|
fn h(f : (String) -> Bool raise?, x : String) -> Bool raise? {
  f(x)
}

///|
fn main {
  let x = try? h(x => !is_file(x), "cmp")
  println(x)
}
