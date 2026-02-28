struct V {
  arr : FixedArray[Int]
  index : Int
}

fn f(v : V) -> Unit {
  println(v.arr[v.index])
}

fn init {
  f({ arr: [ 42 ], index: 0 })
}
