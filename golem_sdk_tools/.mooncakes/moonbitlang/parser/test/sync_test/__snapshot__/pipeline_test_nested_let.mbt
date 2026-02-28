fn f(x: Int) -> Unit {
  let x1 = {
    let x2 = {
      let x3 = x+1
      let x4 = x3*2
      let x5 = x4+2
      x5-3
    }
    let x6 = {
      let x7=x-1
      let x8=x7+x7
      x7*x8
    }
    x2+x6
  }
  println(x1)
}

fn init {
  f(10)
}
