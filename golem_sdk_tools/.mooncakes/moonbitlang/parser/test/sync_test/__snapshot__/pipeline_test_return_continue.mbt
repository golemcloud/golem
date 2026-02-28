///|
fn loop1(x : Int) -> Unit {
  loop x {
    x => {
      if x > 0 {
        return {
          continue x - 1
        }
      }
      println(42)
    }
  }
}

///|
fn main {
  loop1(10)
}
