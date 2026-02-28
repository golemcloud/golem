fn main {
  let a : Int = 10 // no source map
  let b = 20 // no source map
  println(a + b) // no source map

  let mut  c = 10 // no source map
  c = c + 1 // no source map
  println(c) 

  let d = 20 // no source map
  // d = d + 1
  println(d)
}