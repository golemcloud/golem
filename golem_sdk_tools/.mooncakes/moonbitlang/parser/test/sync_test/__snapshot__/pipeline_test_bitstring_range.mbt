//!build: -w -27

fn f1(x: Bytes) -> Unit {
  match x {
    [ u3(a), .. ] => println(a)
    _ => println("f1 other")
  }
}

fn f2(x: Bytes) -> Unit {
  match x {
    [ u3(a) ] => println(a)
    _ => println("f2 other")
  }
}

fn main {
  f1("")
  f2("")
  f1("\xAA")
  f2("\xAA")
  f1("\xAA\xAA")
  f2("\xAA\xAA")

}
