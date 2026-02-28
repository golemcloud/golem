fn a1(arr: FixedArray[Bool]) -> Int {
  let arr = Array::from_fixed_array(arr)
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
  let arr = Array::from_fixed_array(arr)
   match arr {
     [] => 0
     [x] => -x
     [x, y] => x + y
     [.. _, x, y] => y - x
   }
}

fn a3(arr: FixedArray[Int]) -> Unit {
  let arr = Array::from_fixed_array(arr)
   match arr {
     [] => println(0)
     [_] => println(1)
     [_, _] => println(2)
     [_, _, _] => println(3)
     [.. _, x, y, z, w] => { println(x); println(y); println(z); println(w) }
   }
}


fn init {

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

  a3([20, 23, 9, 1])
}
