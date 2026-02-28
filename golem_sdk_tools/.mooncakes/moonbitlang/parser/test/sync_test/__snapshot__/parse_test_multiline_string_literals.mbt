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

fn multiline_string_with_interp1(x : Int, y : Bool) -> String {
  (
    $| Hello, \{y}
    #| String 1
    #| String 2
    $| \t Goodbye, \{f(x + 5)}
    #| String 3
  )
}

fn multiline_string_with_interp2(x : Int, y : Bool) -> String {
  (
    $| no interpolation
    $| Hello, \{y}
    $| \t Goodbye, \{f(x + 5)}
    $|
  )
}
