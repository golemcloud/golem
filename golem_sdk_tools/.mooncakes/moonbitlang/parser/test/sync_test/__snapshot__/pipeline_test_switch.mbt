// constant constructor => int switch
enum E1 {
  A1
  A2
  A3
  A4
  A5
  A6
}

fn f1(e : E1) -> String {
  match e {
    A1 => "A1"
    A2 => "A2"
    A3 => "A3"
    A4 => "A4"
    A5 => "A5"
    A6 => "A6"
  }
}

fn init {
  println(f1(A1))
  println(f1(A3))
  println(f1(A5))
}

// int constant switch
fn f2(x : Int) -> String {
  match x {
    0 => "0"
    1 => "1"
    2 => "2"
    3 => "3"
    4 => "4"
    5 => "5"
    6 => "6"
    _ => "!"
  }
}

fn init {
  println(f2(0))
  println(f2(2))
  println(f2(4))
  println(f2(6))
  println(f2(42))
}

// with payload => constructor switch
enum E2 {
  B1
  B2
  B3
  B4
  B5
  B6(Int)
}

fn f3(e : E2) -> String {
  match e {
    B1 => "B1"
    B2 => "B2"
    B3 => "B3"
    B4 => "B4"
    B5 => "B5"
    B6(42) => "surprise!"
    B6(_) => "B6"
  }
}

fn init {
  println(f3(B1))
  println(f3(B3))
  println(f3(B5))
  println(f3(B6(0)))
  println(f3(B6(42)))
}
