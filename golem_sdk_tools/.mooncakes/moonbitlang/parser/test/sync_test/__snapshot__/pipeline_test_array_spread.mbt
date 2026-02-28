let u1: ReadOnlyArray[Int] = [1, 2, 3]
let u2: ReadOnlyArray[Int] = [5, 6, 7]

fn init {
  let v = [..u1, 4, ..u2, 8]
  println(v)
}

fn init {
  let v = ['1','2','3', .."xx", .."yy", 'z', 'z', .."中午"]
  println(v)
}
