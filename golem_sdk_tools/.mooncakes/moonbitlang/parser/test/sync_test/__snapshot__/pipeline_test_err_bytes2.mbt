fn init{
  let bytes = FixedArray::make(5, (56).to_byte())
  let mut i = 0
  bytes[1] = (57).to_byte();
  bytes[2] = (58).to_byte();

  let len = bytes.length()
  println("length of bytes: \{len}\n")


  while i < 5 {
    let b = bytes[i]
    println("byte \{i}: \{b}\n");
    i = i + 1
  }

  let a = bytes[5] //out of bound
  println(a)
  println("reading from bytes[5] should be failed\n")
  let b = bytes[-1] //out of bound
  println(b)
  println("reading from bytes[-1] should be failed\n")
}
