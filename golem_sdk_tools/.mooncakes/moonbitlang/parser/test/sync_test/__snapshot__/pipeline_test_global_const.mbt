let f = 10.13
let a = 15
let b = 16L
let c = false
fn init {
  println(f)
  println(a)
  println(b)
  println(c)
}

///|
let bi : BigInt = 1

///|
fn init {
  let bbi : BigInt = 2
  println(bi + bbi)
}
