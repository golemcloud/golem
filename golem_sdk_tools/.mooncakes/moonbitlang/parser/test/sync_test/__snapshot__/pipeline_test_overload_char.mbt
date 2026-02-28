///|
fn string_get(s : String, i : Int) -> Int = "%string_get"

///|
fn f1(s : String) -> Bool {
  string_get(s, 0) == 'x'
}

///|
fn f2(s : String) -> Int {
  string_get(s, 0) - 'a'
}

///|
fn f3(x : Int) -> Unit {
  println(x)
}

///|
fn f4(s : String) -> Bool {
  string_get(s, 0) is ('a'..='z')
}

///|
fn main {
  println(f1("abc"))
  println(f1("xxx"))
  println(f2("abc"))
  println(f2("xxx"))
  println(f3('😭'))
  println(f4("abc"))
  println(f4("123"))
}
