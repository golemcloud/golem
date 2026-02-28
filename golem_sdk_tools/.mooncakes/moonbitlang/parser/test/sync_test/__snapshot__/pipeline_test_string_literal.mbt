fn init {
  let  x = "hello你好\u{0000}\u0000" 
  // let y = b"你好"

  let y = b"\x01\x02\x03"
  println(x.to_array())
  println(y.to_array())
  // match y {
  //   [1,2,3, .. ] => println("Matched pattern!"),
  //   _ => println("Did not match.")
  // }
}
