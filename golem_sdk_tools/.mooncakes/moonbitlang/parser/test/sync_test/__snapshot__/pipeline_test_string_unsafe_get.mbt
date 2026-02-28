///|
fn str_unsafe_get(s : String, i : Int) -> UInt16 = "%string.unsafe_get"

///|
fn init {
  let s = "hello😭"
  for i in 0..=6 {
    println(str_unsafe_get(s, i))
  }
}
