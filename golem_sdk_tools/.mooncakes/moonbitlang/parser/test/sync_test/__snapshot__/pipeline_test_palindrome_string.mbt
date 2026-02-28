///|
pub fn palin(p : String) -> Bool {
  loop p.view() {
    [head,  .. middle,  tail] =>
      if head == tail {
        continue middle
      } else {
        return false
      }
    [_] | [] => true
  }
}

///|
fn main {
  println(palin("abba"))
  println(palin(""))
  println(palin("🤣"))
  println(palin("🤣🤣"))
  println(palin("🤣😂🤣"))
  println(palin("🤣😂😂🤣"))
  println(palin("🤣😂🎉😂🤣"))
  println("----")
  println(palin("🤣😂🎉🤣"))
  println(palin("hello"))
  println(palin("abca"))
}
