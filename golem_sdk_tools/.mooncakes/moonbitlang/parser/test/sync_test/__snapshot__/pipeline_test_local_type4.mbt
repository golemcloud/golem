fn main {
  struct S {
    flag : Bool
    y : Int
  } derive(Show)
  let s = { y: 1, flag: true }
  println(s)
}
