fn main {
  pipe_basic()
  pipe_failed()
  pipe_failed2()
  pipe_early_return()
}

fn pipe_basic() -> Unit {
  let r = 0 |> x => { x + 2 }
  println(r == 2)
  let r = 0 |> x => { x } |> y => { y + 3 }
  println(r == 3)
}

suberror MyErr {
  MyErr(String)
} derive(Show)

fn pipe_failed() -> Unit {
  println(try? {
    "msg" |> x => { raise MyErr(x) }
  })
}

fn pipe_failed2() -> Unit {
  println("msg" |> x => { try? (x + "2") |> fail })
  ("msg3" |> x => { fail(x) } |> println) catch { e => println(e.to_string()) }
  "msg4" |> x => { fail(x) catch { _ => x } } |> println
}

fn pipe_early_return() -> Unit {
  "" |> x => { return }
  println("ok")
}





