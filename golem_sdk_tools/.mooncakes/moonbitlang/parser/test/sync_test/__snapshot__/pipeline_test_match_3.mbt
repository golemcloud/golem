
struct T {
  x: Bool
  y: Bool?
}


fn r1(t: T) -> Int {
  match t {
    { x: true , y: None } => 1
    { x: false, y: Some(_) } => 2
    { x: _,     y: Some(false) } => 3
    { x: true , y: Some(true) } => 4
  }
}


fn r2(t: T) -> Int {
  match t {
    { x: true, .. } => 1
    { y: Some(x), .. } => if x { 2 } else { 3 }
    { x: false, y: None } => 4
  }
}

fn a1(arr: FixedArray[Bool]) -> Int {
  match arr {
    [] => 1
    [true] => 2
    [_, false] => 3
    [false] => 4
    [true, _] => 5
    _ => 6
  }
}


fn a2(arr: FixedArray[Int]) -> Int {
   match arr {
     [] => 0
     [x] => -x
     [x, y] => x + y
     [.. _, x, y] => y - x
   }
}

fn a3(arr: FixedArray[Int]) -> Unit {
   match arr {
     [] => println(0)
     [_] => println(1)
     [_, _] => println(2)
     [_, _, _] => println(3)
     [.. _, x, y, z, w] => { println(x); println(y); println(z); println(w) }
   }
}


fn empty(x: Int) -> Int {
  match x {
  }
}

fn init {
  println(r1({ x: true , y: None }))
  println("X") // { x: false, y: None } is unmatched
  println(r1({ x: true , y: Some(true ) }))
  println(r1({ x: true , y: Some(false) }))
  println(r1({ x: false, y: Some(true ) }))
  println(r1({ x: false, y: Some(false) }))
  println("\n===\n")

  println(r2({ x: true , y: None }))
  println(r2({ x: true , y: Some(true ) }))
  println(r2({ x: true , y: Some(false) }))
  println(r2({ x: false, y: None }))
  println(r2({ x: false, y: Some(true ) }))
  println(r2({ x: false, y: Some(false) }))
  println("\n===\n")

  println(a1([]))
  println(a1([true ]))
  println(a1([false]))
  println(a1([true , true ]))
  println(a1([true , false]))
  println(a1([false, true ]))
  println(a1([false, false]))
  println(a1([true, true, true]))
  println("\n===\n")

  println(a2([]))
  println(a2([9]))
  println(a2([2, 3]))
  println(a2([3, 4, 5]))
  println("\n===\n")

  // println(empty(0))
  a3([20, 23, 9, 1])
}
