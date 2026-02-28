suberror StringError {
  StringError(String)
}

fn test_int(x: Int, y: Int) -> Unit raise {
  if x == y {
    println(".")
  } else {
    raise StringError("not equal")
  }
}

// basic usage
fn f(x: Int) -> Int raise StringError {
  if x >= 10 {
    x
  } else {
    raise StringError("less than 10")
  }
}

fn g(x: Int) -> Int raise StringError {
  let a = f(x)
  a+10
}

fn init {
  try! test_int(try! f(42), 42)
  try! test_int(try! g(42), 52)
}

fn init {
  fn f(x: Option[Int]) raise {
    match x {
      Some(i) => i
      None => {
        raise StringError("err")
      }
    }
  }
  try! test_int(try! f(Some(42)), 42)
}

enum T {
  A(Int)
  B(Int)
}

fn T::get_a(self: T) -> Int raise {
  match self {
    A(i) => i
    B(i) => raise StringError("not a")
  }
}

fn get_aa(x: T, y: T) -> Int raise {
  let a1 = x.get_a()
  let a2 = y.get_a()
  a1 + a2
}

fn T::add(self: T, other: T) -> Int raise {
  match (self, other) {
    (A(i), A(j)) => i + j
    _ => raise StringError("not all a")
  }
}

fn add_twice(x: T, y: T) -> Int raise {
  let r1 = x.add(y)
  let r2 = x |> get_aa(y)
  r1 + r2
}

fn init {
  let t = T::A(42)
  try! test_int(try! t.get_a(), 42)
  let r = try! t |> get_aa(t)
  try! test_int(r, 84)
  let t1 = T::A(20)
  let t2 = T::A(22)
  let r = try! t1.add(t2)
  try! test_int(r, 42)
  let r = try! add_twice(t1, t2)
  try! test_int(r, 84)
}

fn infer_raise(x: Int) -> Int raise {
  let a = if x < 10 { raise StringError("err") } else { x + 10 }
  a
}

fn init {
  let x = try! infer_raise(32)
  try! test_int(x, 42)
}
