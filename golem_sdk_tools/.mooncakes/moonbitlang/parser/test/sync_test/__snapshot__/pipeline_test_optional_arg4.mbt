fn hello1(x? : Int, y? : Int) -> Int {
  let x = x.unwrap_or(0)
  let y = y.unwrap_or(0)
  x + y
}

fn init {
  // println(hello(x: 1, y: 2))
  // TODO: should we accept about
  println("Test hello1")
  println(hello1(x=1, y=2))
  println(hello1(x=1))
  println(hello1())
  println(hello1(y=2))
}

fn hello2(x? : Int, y? : Int=1) -> Int {
  let x = x.unwrap_or(0)  
  x + y
}

fn init {
  println("Test hello2")
  println(hello2(x=1, y=2))
  println(hello2(x=1))
  println(hello2())
  println(hello2(y=2))
}

fn hello3(z : Int , x? : Int, y? : Int=z) -> Int {
  let x = x.unwrap_or(z)
 
  x + y + z
}

fn init {
  println("Test hello3")
  println(hello3(1, x=2, y=3))
  println(hello3(1, x=2))
  println(hello3(1))
}

fn hello4(x : Int, y? : Int=10, z? : Int=y) -> Int {
  x + y + z
}

fn init {
  println("Test hello4")
  println(hello4(1))
  println(hello4(1, y=2))
  println(hello4(1, y=2, z=3))
}