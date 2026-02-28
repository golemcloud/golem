fn f(x : Int) -> String raise {
  let m = { "x": 1, "y": 2 }
  for _, _ in m {
    while true {
      match x {
        0 => return "return"
        1 => fail("error")
        _ => break
      }
    }
    break
  }
  "normal"
}

fn init {
  println(try? f(0))
  println(try? f(1))
  println(try? f(2))
}
