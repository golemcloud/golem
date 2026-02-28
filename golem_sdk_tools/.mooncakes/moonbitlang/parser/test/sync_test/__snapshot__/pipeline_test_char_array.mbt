///|
fn f(s : String) -> Unit {
  println(s)
}

///|
fn main {
  let c = 'a'
  let cx = '🤔'
  f(['a', 'b', 'c', 'd'])
  f(['a', '😭', 'c', '😭', 'e'])
  f([c, c, c])
  f(['😭', c, cx, c, '😭'])
  f([])
}
