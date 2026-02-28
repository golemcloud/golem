fn init{
  let bytes = FixedArray::make(3, (56).to_byte())
  let mut i = 0
  bytes[1] = (57).to_byte();
  bytes[2] = (58).to_byte();

  let len = bytes.length()
  println("length of bytes: \{len}\n")


  while i < 3 {
    let b = bytes[i]
    println("byte \{i}: \{b}\n");
    i = i + 1
  }

  bytes[3] = (57).to_byte() //out of bound
  println("access to bytes[3] should be failed")
  bytes[-1] = (57).to_byte() //out of bound
  println("access to bytes[-1] should be failed")
}
