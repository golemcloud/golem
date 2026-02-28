struct H {
  x:U
}

struct U {
  mut x:X
}

struct X {
  mut body:Int
}

fn hello(x: H) -> Unit {
  x.x.x.body = 3
}

fn init {
  fn test1(x : FixedArray[FixedArray[FixedArray[FixedArray[Int]]]]) {
    x[0][0][1][2] = 2
  }

  let x: FixedArray[FixedArray[FixedArray[FixedArray[Int]]]] = [[[[1, 2, 3], [3, 4, 5]]]]
  test1(x)
  println(x[0][0][1][2])
}

