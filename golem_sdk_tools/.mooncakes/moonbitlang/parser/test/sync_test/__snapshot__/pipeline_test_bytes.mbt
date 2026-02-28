///|
fn to_be_bytes(n : Int) -> Bytes {
  [
    (n >> 24).to_byte(),
    (n >> 16).to_byte(),
    (n >> 8).to_byte(),
    n.to_byte(),
  ]
}

///|
fn test_get_item1(x : Bytes) -> Unit {
  println(x[0])
  println(x[1])
}

///|
fn test_get_item2(x : FixedArray[Byte]) -> Unit {
  println(x[1])
  println(x[2])
}

///|
fn test_get_item3(x : FixedArray[Byte]) -> Unit {
  match x {
    [a0, .. rest, aN] => {
      println(a0)
      println(aN)
      println(rest)
    }
    [] => println("empty")
    [a] => println(a)
  }
}

///|
fn test_get_item4(x : Bytes) -> Unit {
  match x {
    [a0, .. rest, aN] => {
      println(a0)
      println(aN)
      println(rest)
    }
    [] => println("empty")
    [a] => println(a)
  }
}

///|
fn test_length1(x : Bytes) -> Unit {
  println(x.length())
}

///|
fn test_length2(x : FixedArray[Byte]) -> Unit {
  println(x.length())
}

///|
fn test_set_item(x : FixedArray[Byte]) -> Unit {
  x[0] = 0x34
  println(x)
}

///|
fn test_make1(len : Int, init : Byte) -> Unit {
  let x = FixedArray::make(len, init)
  println(x)
}

///|
fn test_make2(len : Int, init : Byte) -> Unit {
  let x = Bytes::make(len, init)
  println(x)
}

///|
fn test_make3(b1 : Byte, b2 : Byte, b3 : Byte) -> Unit {
  let x : Bytes = [b1, b2, b3, 0x34, 0x35]
  println(x)
  let x : FixedArray[Byte] = [b1, b2, b3, 0x34, 0x35]
  println(x)
}

///|
fn test_array_of_bytes() -> Unit {
  let x = []
  for i in 0..<10 {
    x.push(to_be_bytes(i))
  }
  println(x)
  for i in 0..<5 {
    let _ = x.pop()

  }
  println(x)
}

///|
fn test_bytes_fill() -> Unit {
  let bytes : FixedArray[Byte] = FixedArray::make(10, 0x00)
  bytes.fill(0x31, start=2, end=5)
  println(bytes)
}

///|
fn test_bytes_copy() -> Unit {
  let bytes1 : FixedArray[Byte] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a,
  ]
  let bytes2 = bytes1.copy()
  println(bytes2)
  let bytes3 : FixedArray[Byte] = FixedArray::make(10, 0x00)
  bytes3.unsafe_blit(1, bytes1, 4, 3)
  println(bytes3)
}

///|
fn main {
  println("test_get")
  test_get_item1(b"123")
  test_get_item2([0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a])
  test_get_item3([0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a])
  test_get_item3([])
  test_get_item3([0x31])
  test_get_item4(b"123456789a")
  test_get_item4(b"")
  test_get_item4(b"a")
  println("test_length")
  test_length1(b"123")
  test_length2([0x31, 0x32, 0x33])
  println("test_set_item")
  test_set_item([0x31, 0x32, 0x33])
  println("test_make")
  test_make1(5, 0x34)
  test_make2(5, 0x35)
  test_make3(0x31, 0x32, 0x33)
  println("test array of bytes")
  test_array_of_bytes()
  println("test bytes fill")
  test_bytes_fill()
  println("test bytes copy")
  test_bytes_copy()
}
