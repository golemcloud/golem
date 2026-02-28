///|
fn contains(s : StringView, c : Char) -> Bool {
  for a in s {
    if a == c {
      return true
    }
  } else {
    false
  }
}

///|
fn main {
  println(contains("hello", 'e'))
  println(contains("xxx" + "hello", 'e'))
  println(contains(String::make(5, 'a'), 'e'))
}
