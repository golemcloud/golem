fn continue_in_for(n: Int) -> Unit {
  for i = 0; i < n; i = i + 1 {
    if i % 3 == 0 {
      continue i + 2
    } else if i % 5 == 0 {
      continue
    }
    println(i)
  }
}

fn continue_in_for2(n: Int) -> Unit {
  for i = 0, s = ""; i + s.length() < n; i = i + 1, s = s + i.to_string() {
    if i % 3 == 0 {
      continue i + 2, s + "!"
    } else if i % 5 == 0 {
      continue
    }
    println(s)
  }
}

fn continue_in_for3(n: Int) -> Unit {
  let mut res = 0
  for i = 0; i < n; i = i + 1 {
    let sum = loop (0, 0) {
      (j, acc) => if j >= i { break acc } else { continue (j + 2, acc + j) }
    }
    if sum == res {
      continue i + 3
    } else {
      println(sum)
      res = sum
    }
  }
}

fn init {
  continue_in_for(10)
  continue_in_for2(20)
  continue_in_for3(10)
}
