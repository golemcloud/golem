trait Add {
  f(x : Int, y : Int) -> Int // currently it still works without Self type
}


#cfg(false)
fn main {
  println((&Add::f)(1, 2))
}

#cfg(true)
fn main {
  println("OK")
}