suberror Error1 {
  Error1(String)
}

suberror Error2 {
  Error2(String)
}

fn f1() -> Unit raise Error1 {
  raise Error1("xxx")
}

fn f2() -> Unit raise Error2 {
  raise Error2("yyy")
}

fn f(b: Bool) -> String {
  try {
    if b { f1() } else { f2() }
    "www"
  } catch {
    Error1(s) => s
    Error2(s) => s
    _ => "zzz"
  }
}

fn init {
  println(f(true))
  println(f(false))
}
