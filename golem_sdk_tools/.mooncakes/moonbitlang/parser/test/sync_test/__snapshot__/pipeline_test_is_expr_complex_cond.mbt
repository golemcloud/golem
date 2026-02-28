///|
enum E {
  A(Int)
  B(Int)
  C(Int)
}

fn f1(x: E) -> Unit {
  guard (x is A(a) && a >= 10) || (x is B(b) && b >= 20) || (x is C(c) && c >= 30) else {
    println("f1 ifnot")
  } 
  println("f1 ifso")
}

fn f2(x1: E, x2: E) -> Unit {
  if (x1 is A(_) || x1 is B(_)) && (x2 is B(_) || x2 is C(_)) {
    println("f2 ifso")
  } else {
    println("f2 ifnot")
  }
}

///|
fn g(x : Bool) -> Unit {
  println(x)
}

fn f3(x: E) -> Unit {
  g((x is A(a) && a >= 10) || (x is B(b) && b >= 20) || (x is C(c) && c >= 30))
}

fn f4(x1: E, x2: E) -> Unit {
  g((x1 is A(_) || x1 is B(_)) && (x2 is B(_) || x2 is C(_)))
}

///|
fn main {
  println("test f1")
  f1(A(10))
  f1(B(20))
  f1(C(30))
  f1(A(9))
  f1(B(19))
  f1(C(29))
  println("test f2")
  f2(A(1), B(2))
  f2(B(2), C(3))
  f2(B(2), A(1))
  f2(C(3), B(2))
  println("test f3")
  f3(A(10))
  f3(B(20))
  f3(C(30))
  f3(A(9))
  f3(B(19))
  f3(C(29))
  println("test f4")
  f4(A(1), B(2))
  f4(B(2), C(3))
  f4(B(2), A(1))
  f4(C(3), B(2))
}
