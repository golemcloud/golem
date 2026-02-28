fn sign(x : Int) -> Int {
  match x {
    _..<0 => -1
    0 => 0
    0..<_ => 1
  }
}

fn init {
  println("sign(1) = \{sign(1)}")
  println("sign(0) = \{sign(0)}")
  println("sign(-1) = \{sign(-1)}")
}

fn classify_char(c : Char) -> String {
  match c {
    'a'..='z' => "lowercase"
    'A'..='Z' => "uppercase"
    '0'..='9' => "digit"
    _ => "other"
  }
}

fn init {
  println("classify_char('z') = \{classify_char('z')}")
  println("classify_char('X') = \{classify_char('X')}")
  println("classify_char('3') = \{classify_char('3')}")
  println("classify_char('字') = \{classify_char('字')}")
}

fn overlap(x : UInt) -> Int {
  match x {
    100..<200 => 1
    0..<150 => 2
    200..<_ => 3
  }
}

fn init {
  println(overlap(50))
  println(overlap(100))
  println(overlap(150))
  println(overlap(200))
}


const Zero = 0
fn is_positive(x : Int) -> Bool {
  match x {
    Zero ..< _ => true
    _ ..< Zero => false
  }
}

fn init {
  println(is_positive(1))
  println(is_positive(-1))
}

const UZero : UInt = 0
fn is_positive_unsigned_int(x : UInt) -> Bool {
  match x {
    1 ..< _ => true
    UZero => false
  }
}

fn init {
  println(is_positive_unsigned_int(1))
  println(is_positive_unsigned_int(0))
}