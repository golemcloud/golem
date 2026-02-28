fn expect_int(i: Int, e: Int) -> Unit {
  if i == e {
    println(".")
  } else {
    println("x")
  }
}

fn f1(x: String?) -> Int {
  match x {
    Some(s) => s.length()
    None => -1
  }
}

fn f2(x: String??) -> Int {
  match x {
    Some(Some(s)) => s.length()
    Some(None) => -1
    None => -2
  }
}

fn init {
  expect_int(f1(Some("42")), 2)
  expect_int(f1(None), -1)
  expect_int(f2(Some(Some("1234"))), 4)
  expect_int(f2(None), -2)
  expect_int(f2(Some(None)), -1)
}

fn f3(xs: Array[String?]) -> Int {
  let mut res = 0
  xs.iter().each(fn (x) {
    match x {
      Some(s) => res = res + s.length()
      None => ()
    }
  })
  res
}

fn init {
  expect_int(f3([Some("abc"), None, Some("def")]), 6)
}

struct S {
  x: String?
  y: String
}

fn f4(a: S?, b: Int) -> (Int) -> Int {
  fn aux(c: Int) -> Int {
    match a {
      Some(s) => {
        match s.x {
          Some(i) => i.length() + b + c
          None => 100 + s.y.length() + b + c
        }
      }
      None => 1000 + c
    }
  }
  aux
}

fn init {
  expect_int(f4(Some({ x: Some("123"), y: "456"}), 5)(3), 11)
  expect_int(f4(Some({ x: None, y: "456"}), 5)(3), 111)
  expect_int(f4(None, 5)(3), 1003)
}

let x1: T? = f5(-42)
let x2: T? = f5(0)

enum T {
  Pos(Int)
  Neg(Int)
}

fn f5(x: Int) -> T? {
  if x > 0 {
    Some(Pos(x))
  } else if x < 0 {
    Some(Neg(x))
  } else {
    None
  }
}

fn init {
  let n = match (x1, x2) {
    (Some(Neg(a)), None) => -a
    _ => 43
  }
  expect_int(n, 42)
}

fn f6(x: Char) -> Int? {
  if x.compare('a') > 0 {
    Some(x.to_int())
  } else {
    None
  }
}

fn f7(x: Int) -> String? {
  if x > 'b'.to_int() {
    Some("ok")
  } else {
    None
  }
}

fn f8(x: String) -> FixedArray[Char]? {
  if x == "ok" {
    Some(['o', 'k'])
  } else {
    None
  }
}
