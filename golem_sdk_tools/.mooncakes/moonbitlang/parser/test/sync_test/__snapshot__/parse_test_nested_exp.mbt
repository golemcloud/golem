struct H {
  mut x:U
}

struct U {
  mut x:Int
}

fn hello(x: H) {
  x.x.x = 3
}

fn init {
  fn test1(x) {
    x[0][1][2] = 2
  }

  let x = [[[1, 2], [3, 4]]]
  test1(x)
  output_int(x[0][1][2])
}

