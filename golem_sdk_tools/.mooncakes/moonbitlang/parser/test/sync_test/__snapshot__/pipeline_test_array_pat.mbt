
fn f1(arr: FixedArray[Bool]) -> Int {
  match arr {
    [] => 1
    [true] => 2
    [_, false] => 3
    [false] => 4
    [true, _] => 5
    _ => 6
  }
}

fn init {
  println(f1([]))
  println(f1([true ]))
  println(f1([false]))
  println(f1([true , true ]))
  println(f1([true , false]))
  println(f1([false, true ]))
  println(f1([false, false]))
  println(f1([true, true, true]))
  println("\n===\n")
}
