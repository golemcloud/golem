suberror E1 derive(Show)
suberror E2 derive(Show)

fn f1() -> Unit raise E1 { raise E1 }
fn f2() -> Unit raise E2 { raise E2 }

fn f3() -> Unit raise {
  try {
    f1()
    f2()
  } catch {
    E1 => println("E1")
    e => raise e
  }
}

fn f4() -> Unit raise {
  try {
    f2()
    f1()
  } catch {
    E1 => println("E1")
    e => raise e
  }
}

fn main {
  println(try? f3())
  println(try? f4())
}
