suberror Error1 {
  A
  B(Int, x~: String)
  C(mut x~: String, Char, y~: Bool)
} derive(Show)

suberror Error2 {
  A
  B(Int, x~: String)
  C(mut x~: String, Char, y~: Bool)
} derive(Show)

fn f1() -> Unit raise Error1 {
  raise Error1::B(42, x="error1")
}

fn f2() -> Unit raise Error2 {
  raise Error2::C('x', x="error2", y=false)
}


fn t1() -> Unit {
  try f1() catch {
    A => println("A")
    B(i, ..) as b => {
      println(i)
      println(b.x)
      println(b)
    }
    C(t, ..) as c => {
      println(t)
      println(c.x)
      println(c)
    }
  }
}

fn t2() -> Unit raise Error2 {
  try { f2(); f1() } catch {
    Error1::A => println("Error1::A")
    Error1::B(_) => println("Error1::B")
    Error1::C(_) => println("Error1::C")
    Error2::A => println("Error2::A")
    Error2::B(_) => println("Error2::B")
    Error2::C(_) as c => {
      println("Error2::C")
      println(c)
      c.x = "Rethrow Error2::C"
      raise c
    }
    _ => println("unknown error")
  }
}

fn main {
  t1()
  try t2() catch {
    Error2::C(_) as c => println(c)
    _ => println("unknown error")
  }
}
