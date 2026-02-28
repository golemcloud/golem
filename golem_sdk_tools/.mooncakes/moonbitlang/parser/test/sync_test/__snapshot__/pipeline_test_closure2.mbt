let arr : FixedArray [(Int) -> Int] = [id]

fn id(x: Int) -> Int {
  x
}

fn init {
  println(arr[0](1))
}
