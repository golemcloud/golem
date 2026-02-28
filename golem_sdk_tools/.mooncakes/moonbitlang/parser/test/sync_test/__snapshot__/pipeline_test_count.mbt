struct T {
  mut value:Int
}

fn sum(x: FixedArray[Int]) -> Int {
  let u = { value:0, }
  let mut i = 0
  while i < x.length() {
    u.value = u.value + x[i]
    i = i + 1
  }
  u.value
}

fn init {
  println(sum([1, 2, 3, 4, 5]))
}

