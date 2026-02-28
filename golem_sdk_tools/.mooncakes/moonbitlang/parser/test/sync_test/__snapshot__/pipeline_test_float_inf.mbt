fn main {
  let s = 1.0e+600
  println(s)
  let s = -1.0e+600
  println(s)
  let s: Float = 1.0e+600
  println(s.to_double())
  let s: Float = -1.0e+600
  println(s.to_double())
}
