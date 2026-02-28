///|
fn f(view : StringView) -> Unit {
  match view {
    ['🤣', .. rest] => {
      println("first")
      println(rest)
    }
    [.. rest, '🤣'] => {
      println("last")
      println(rest)
    }
    [a,  .. rest,  b] => {
      if a == b {
        println("head and tail are the same")
      }
      println(rest)
    }
    [x] => {
      println("singleton")
      println(x)
    }
    _ => println("match failed")
  }
}

///|
fn p(view : StringView) -> Bool {
  match view {
    [] | [_] => true
    [a,  .. rest,  b] => a == b && p(rest)
  }
}

///|
fn main {
  f("🤣hello".view())
  f("hello🤣".view())
  f("hello".view())
  f("helloh".view())
  f("😂".view())
  let s = "xx🤣🤣🤣xx"
  println(p("xx🤣🤣🤣xx".view()))
  println(p("xx🤣🤣🤣xx".view(start_offset=1)))
  println(p("xx🤣🤣🤣xx".view(end_offset=9)))
  println(p("xx🤣🤣🤣xx".view(start_offset=1, end_offset=9)))
  println(p("xx🤣🤣xx".view()))
  println(p("xx🤣xx".view()))
  println(p("xx😂🤣xx".view()))
}
