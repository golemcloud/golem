fn init {
  // let s = "\uD835\uDD04"
  let s = "\u{1D504}"
  println(s)
  println(s.length()) 
  // returns 2 
  // since js returns the length of code array
  for i in s {
    println(i)
  }
}
