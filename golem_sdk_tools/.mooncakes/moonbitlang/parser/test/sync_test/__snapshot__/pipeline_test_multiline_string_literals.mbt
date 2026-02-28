//!build: -w -27
fn hello() -> String {
  #| Hello,
  #| World.
  #|
}
let str =
  #| Hello,
  #| World.
  #|
fn init {
  let s =
    #| Hello,
    #| World.
    #|
  println(s)
  println(str)
  println(hello())
}
