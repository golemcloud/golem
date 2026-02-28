///|
fn f(xs : Array[Int]) -> Int {
  xs.binary_search_by(a => a.compare(2)).unwrap()
}

///|
#locals(f)
fn h(xs : Array[Int], f : (Int) -> Int) -> Int {
  let mut res = 0
  for x in xs {
    res = res + f(x) + f(x)
  } else {
    res = res + f(xs.length())
  }
  res
}

///|
#locals(f)
#inline(never)
fn h_never(xs : Array[Int], f : (Int) -> Int) -> Int {
  let mut res = 0
  for x in xs {
    res = res + f(x) + f(x)
  } else {
    res = res + f(xs.length())
  }
  res
}

///|
#inline(never)
fn cb(x : Int) -> Int {
  x * 2
}

///|
fn main {
  println(f([1, 2, 3]))
  println(h([3, 2, 1], cb))
  println(
    h([3, 2, 1], x => {
      println(x)
      x
    }),
  )
  println(h_never([3, 2, 1], cb))
  println(
    h_never([3, 2, 1], x => {
      println(x)
      x
    }),
  )
}
