fn init {
  // oop style method call
  println(to_string([1, 2, 3]))
}


fn[T : Show] to_string(xs: FixedArray[T]) -> String {
  let mut str = ""
  for i = 0; i < xs.length(); i = i + 1 {
    str = str + xs[i].to_string()
  }
  str
}
