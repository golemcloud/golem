let test1:(Bool) -> Unit = fn(b) {
  if b { println("ok\n") } else { println("not ok\n") }
}

struct T1 {
  x:Int
  y:Int
}

struct T2 {
  x:Int
  y:Int
  z:Int
}

fn init {
  let a = { x:1, y:2 }
  let b = { x:3, y:4, z:5 }
  test1(a.x == 1)
  test1(a.y == 2)
  test1(b.x == 3)
  test1(b.y == 4)
  test1(b.z == 5)
}

fn t1_of_t2(x: T2) -> T1 {
  let x = match x {
    {x:a, y:b, z:c} => { x:a, y:b }
  }
  x
}

fn init {
  let x = { x:3, y:4, z:5 }
  let y = t1_of_t2(x)
  test1(y.x == 3)
  test1(y.y == 4)
}

