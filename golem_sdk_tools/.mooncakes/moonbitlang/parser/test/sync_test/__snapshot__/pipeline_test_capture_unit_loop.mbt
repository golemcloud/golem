fn init {
  let x = loop 50 {
    0 => ()
    n => {
      if n == 42 {
        break
      }
      println(n)
      continue n - 1
    }
  }
  println(x)
}
