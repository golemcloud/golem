struct T[X] {
  mut sum : X
  mut num : Int
}

fn[X : Add] T::add(self : T[X], x : X) -> Int {
  self.sum += x
  self.num += 1
  self.num - 1
}

fn init {
  let a = { sum: 0, num: 0 }
  let x = FixedArray::make(6, 10)
  let y = FixedArray::make(6, 10)
  let mut t = x
  fn next() {
    if physical_equal(t , x) {
      t = y
    } else {
      t = x
    }
    t
  }

  next()[a.add(1)] *= 1
  next()[a.add(1)] *= 2
  next()[a.add(4)] *= 3
  next()[a.add(5)] *= 4
  next()[a.add(1)] *= 5
  next()[a.add(4)] *= 6
  println(a.sum)
  println(a.num)
  println(x)
  println(y)
}

