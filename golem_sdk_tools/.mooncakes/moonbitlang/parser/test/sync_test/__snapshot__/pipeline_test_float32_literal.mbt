fn main {
  let f = 3.14F
  println((f : Float))
  println(1.0F)
  println(1.0e2F)
  println(0xABCD.EFp1F)
  match f {
    3.14F => println("3.14")
    _ => println("error")
  }
}
