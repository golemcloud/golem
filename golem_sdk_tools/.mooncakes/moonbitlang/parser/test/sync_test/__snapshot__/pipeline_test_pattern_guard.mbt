fn f1(x: Int??) -> Unit {
  match x {
    Some(y) if y is Some(z) && z >= 0 => {
      println(y)
      println(z)
    }
    Some(_) => println("some")
    None => println("none")
  }
}

fn f3(x: Int, y: Int) -> Unit {
  match (x, y) {
    (a, b) if a > 0 && b > 0 => println("a > 0 && b > 0")
    (a, _b) if a > 0 => println("a > 0")
    (_a, b) if b > 0 => println("b > 0")
    _ => println("a <= 0 && b <= 0")
  }
}

fn f4(xs: Array[Int?]) -> Unit {
  match xs {
    [a, b, .._] if a is Some(c) && b is Some(d) && c + d > 10 => println(c + d)
    [None, a, b, .._] if a is None && b is Some(c) => println(c)
    arr if arr.length() >= 3 => println("len >= 3")
    _ => println("else")
  }
}

fn f5(x: Int) -> Unit {
  let mut cnt = 1000

  fn f() {
    cnt += 1
    println(cnt)
    false
  }

  match x {
    _ if f() => println("1")
    _ if f() => println("2")
    _ if f() => println("3")
    _ if f() => println("4")
    _ as x => println(x)
  }

}

///|
fn main {
  f1(Some(Some(1)))
  f1(Some(Some(-1)))
  f1(Some(None))
  f1(None)
  f3(1, 2)
  f3(1, -2)
  f3(-1, 2)
  f3(-1, -2)
  f4([Some(11), Some(2), Some(3), Some(4)])
  f4([None, None, Some(1), Some(2)])
  f4([None, None, None, Some(1)])
  f4([])
  f5(42)
}
