//!build: -w -52
fn basic(n: Int) -> Unit {
   for i = 0; i < n; i = i + 1 {
     println(i)
   }
}

fn nested(n: Int, s: String) -> Unit {
  for i = 0; i < n; i = i + s.length() {
    for a = i.to_string(); a.length() <= i; a = a + s {
      println(a)
    }

  }
}

fn multi_binder(n: Int) -> Unit {
  let res = FixedArray::make(n, 0)
  for i = 0, x = 0, y = 1
      i < n
      x = y, y = res[i] + y, i = i + 1 {
    res[i] = x
  }
  for i = 0; i < n; i = i + 1 {
    println(res[i])
    println(" ")
  }

}

fn for_return(n: Int?) -> String {
  fn next (x) {
    match x {
      Option::Some(i) => Option::Some(i+1)
      Option::None => Some(0)
    }
  }

  for i = n; ; i = next(i) {
    match i {
      None => return "none"
      Some(j) => {
        for k = 0; k < 100; k = k + j {
          if k > 50 { return k.to_string() }
        }
        return j.to_string() + "!"
      }
    }
  }
  abort("unreachable")
}

fn for_break_continue(xs: FixedArray[Int]) -> Int {
  let mut sum = 0
  for i = 0; i < xs.length(); i = i + 1 {
    if i % 2 == 0 { continue }
    if i == 3 { continue }
    if i >= 7 { break }
    sum = sum + xs[i]
  }
  sum
}

fn simultaneous_update(n: Int) -> String {
  for i = 0, p = "1", q = "2", r = "3", res = "";
      ;
      i = i + 1, p = q, q = p, res = res + p + q + r {
      if i > n { return res }
  }
  abort("unreachable")
}

fn init {
  basic(5)
  nested(5, "!")
  multi_binder(10)
  println(for_return(None))
  println(for_return(Some(17)))
  println(for_return(Some(117)))
  println(for_break_continue(FixedArray::make(10, 10)))
  println(simultaneous_update(5))
}
