fn f1(x: Int?) -> Unit {
  if x is Some(y) && y > 10 {
    println("y > 10")
  } else {
    println("y <= 10")
  }
}

///|
fn f_bool(x : Int?) -> Bool {
  x is None
}

///|
fn f_opt(x : Int) -> Int? {
  if x > 10 {
    Some(x)
  } else {
    None
  }
}

///|
fn f_opt_opt(x : Int) -> Int?? {
  if x > 10 {
    Some(Some(x))
  } else {
    None
  }
}

fn f2(x: Array[Int]) -> Unit {
  if x is [ a ] && a is 10..=20 {
    println(a)
  } else if x is [ a, b ] && (a + b) is 10..=20 {
    println(a+b)
  } else {
    println("else")
  }
  
}

///|
fn main {
  f1(Some(11))
  f1(None)
  f1(Some(9))
  println(f_bool(None) && f_opt(15) is Some(x) && x > 20)
  println(f_bool(None) && f_opt(42) is Some(x) && x > 20)
  println((f_bool(None) && f_opt(42) is Some(x)) && x > 20)
  println((f_bool(None) && f_opt_opt(42) is Some(x)) && x is Some(y) && y > 20)
  f2([0])
  f2([15])
  f2([7, 8])
}
