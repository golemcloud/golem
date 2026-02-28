fn init {
  // array literal
  for x in [1, 2, 3] {
    println(x)
  }
}

fn init {
  // bytes
  for x in b"1234" {
    println(x)
  }
}

fn init {
  // fixedarray
  for x in ([1, 2, 3] : FixedArray[_]) {
    println(x)
  }
}

fn init {
  // arrayview
  let arr = [1, 2, 3, 4, 5]
  for x in arr[1:4] {
    println(x)
  }
}

fn init {
  // array literal, two binders
  for i, x in [1, 2, 3] {
    println((i, x))
  }
}
