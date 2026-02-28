///|
fn f1(s : String, i : Int) -> Unit {
  match (s, i) {
    ("0", 42) => println("0")
    (['a'..='z', _, .._], 24) => println("a-z24 and length > 1")
    _ => println("other")
  }
}

///|
fn f2(s : String, i : Int) -> Unit {
  match (s, i) {
    (['a'..='z', _, .._], 24) => println("a-z24 and length > 1")
    ("0", 42) => println("0")
    _ => println("other")
  }
}

///|
fn f3(s : String, i : Int) -> Unit {
  match (s, i) {
    ("🤣", 42) => println("🤣")
    (['a'..='z', _, .._], 24) => println("a-z24 and length > 1")
    _ => println("other")
  }
}

///|
fn f4(s : String, i : Int) -> Unit {
  match (s, i) {
    ("01", 42) => println("01")
    (['a'..='z', _, .._], 24) => println("a-z24 and length > 1")
    _ => println("other")
  }
}

///|
fn f5(s : String, i : Int) -> Unit {
  match (s, i) {
    ("012", 42) => println("01")
    (['a'..='z', _, .._], 24) => println("a-z24 and length > 1")
    _ => println("other")
  }
}

///|
fn main {
  f1("0", 42)
  f1("abc", 24)
  f1("a", 24)
  f2("0", 42)
  f2("abc", 24)
  f2("a", 24)
  f3("🤣", 42)
  f4("01", 42)
  f5("012", 42)
}
