fn fib( n : Int ) -> Int {
    for i = 0, a = 1, b = 2 ; i < n ; i = i + 1, a = b, b = a + b {
    } else { b }
}

fn in_range(begin: Int, end: Int, number: Int) -> Bool {
    let mut i = begin
    return (while i < end {
        if (i == number) {
            break true;
        }
        i = i + 1
    } else { false })
}

fn[T] find_in_sorted(xs: FixedArray[(Int, T)], i: Int) -> T? {
  for l = 0, r = xs.length() - 1; l < r; {
    let mid = (l + r) / 2
    let k = xs[mid].0
    if k == i {
      break Some(xs[mid].1)
    } else if k > i {
      continue l, mid
    } else {
      continue mid + 1, r
    }
  } else {
    None
  }
}

fn init {
  println(fib(10))
  println(in_range(0, 10, 5))
  println(in_range(0, 10, 15))
}

fn init {
  let xs = FixedArray::make(10, (0, ""))
  for i = 0, j = 1; i < xs.length(); i = i + 1, j = j + i {
    xs[i] = (j, j.to_string())
  }
  println(xs)
  println(find_in_sorted(xs, -1))
  println(find_in_sorted(xs, 5))
  println(find_in_sorted(xs, 16))
  println(find_in_sorted(xs, 44))
}
