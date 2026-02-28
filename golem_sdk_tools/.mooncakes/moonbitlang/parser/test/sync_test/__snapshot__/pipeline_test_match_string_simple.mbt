///|
fn f3(s : String) -> Unit {
  match s {
    ['a', .. rest] => println(rest)
    ['0'..='9' as c, .. rest] => {
      println(c)
      println(rest)
    }
    [c, .. rest] => {
      println(c)
      println(rest)
    }
    _ => println(s)
  }
}

///|
fn main {
  f3("hhha")
  f3("ahhh")
  f3("1234")
  println("done")
}
