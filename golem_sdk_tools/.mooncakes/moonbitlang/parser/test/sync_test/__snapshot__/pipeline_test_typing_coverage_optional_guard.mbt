enum Value {
  IntVal(Int)
  StrVal(String)
  Pair(Int, Int)
}

fn sum(x : Int, y~ : Int = 1, z~ : Int = 2) -> Int {
  x + y + z
}

fn classify(v : Value) -> Int {
  match v {
    IntVal(n) as whole if n > 0 => {
      let _ = whole
      n
    }
    StrVal(s) if s.length() > 0 => s.length()
    Pair(a, b) if a == b => a
    _ => 0
  }
}

fn init {
  let _ = sum(1)
  let _ = sum(1, y=2)
  let _ = sum(1, z=3)
  let _ = sum(1, y=2, z=3)
  let _ = classify(IntVal(5))
  let _ = classify(StrVal("hi"))
  let _ = classify(Pair(3, 3))
  ()
}
