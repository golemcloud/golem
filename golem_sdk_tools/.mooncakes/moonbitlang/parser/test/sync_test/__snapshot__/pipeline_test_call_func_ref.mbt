///|
let global_f : FuncRef[() -> Unit] = () => println("global")

///|
fn main {
  let local_f : FuncRef[(String) -> Unit] = msg => println("local: \{msg}")
  let dynamic_f : Array[FuncRef[(Int) -> Unit]] = [
    x => println("func #1: \{x}"),
    x => println("func #2: \{x}"),
    x => println("func #3: \{x}"),
  ]
  println(global_f())
  local_f("message")
  for i, f in dynamic_f {
    f(i + 1)
  }
}
