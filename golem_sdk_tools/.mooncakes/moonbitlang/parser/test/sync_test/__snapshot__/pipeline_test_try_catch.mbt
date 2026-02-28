suberror StringError {
  StringError(String)
} derive(Show)

fn f(x: Int) -> Int raise StringError {
  if x >= 0 {
    return x
  } else {
    raise StringError("x must be non-negative")
  }
}

fn g(x: Int) -> Int {
  try { f(x) } catch { s => { println(s); 42 } }
}

fn h(x: Int) -> Int {
  try {
    if x >= 0 {
      x
    } else {
      raise StringError("x must be non-negative")
    }
  } catch { s => { println(s); 42 } }
}

fn init {
  let a = g(-1)
  println(a)
  let b = g(42)
  println(b)
  let c = h(-1)
  println(c)
  let d = h(42)
  println(d)
}

enum T {
  A
  B
  C
}

suberror TError {
  TError(T)
}

fn f1(x: T) -> Int raise {
  let a =
    try {
      raise TError(x)
    } catch {
      TError(A) => 1
      TError(B) => raise StringError("got b")
      TError(C) => 42
    }
  a+1
}

fn init {
  try {
    println(f1(T::A))
    println(f1(T::C))
    println(f1(T::B))
  } catch {
    StringError(e) => println(e)
    _ => ()
  }
}
