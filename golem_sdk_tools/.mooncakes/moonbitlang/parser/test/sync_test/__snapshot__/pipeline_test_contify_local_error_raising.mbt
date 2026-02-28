///|
fn main {
  pipe_failed()
}

///|
suberror MyErr {
  MyErr(String)
} derive(Show)

///|
fn pipe_failed() -> Unit {
  println(try? (x => raise MyErr(x))("msg"))
}
