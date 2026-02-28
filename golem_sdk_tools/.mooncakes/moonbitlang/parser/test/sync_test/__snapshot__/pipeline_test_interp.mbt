fn init {
  let mut x = 0
  let mut y = 1
  let mut i = 0
  println("Fib: ")
  while i < 20 {
    let xy = x + y
    println("\{x} + \{y} = \{xy}\n")
    x = y
    y = xy
    i = i + 1
  }
}


fn init {
  let 中文 = 1688
  let s = "hello \{ 中文 }"
  println(s)
}

fn init {
  println("\{1}, \{true}, \{[1,2,3,4,5,6]}, \{'C'}, \{3.14}")
}

fn init {
  let x = 100
  let y = 200
  fn inc(x : Int) -> Int {
    x + 1
  }
  println("x = \{x}, y = \{y}, x + y = \{x + y}")
  println("x = \{x}, y = \{y}, x + y = \{ x.add(y) }")
  println("x + 1 = \{ inc(x) }, (x + 1) * (y + 1) = \{ inc(x) * (y + 1) }")
}

struct Point {
  x : Int
  y : Int
}

fn init {
  let p = { x: 1, y: 2 }
  println("Point: (\{p.x}, \{p.y as &Show})")
}
