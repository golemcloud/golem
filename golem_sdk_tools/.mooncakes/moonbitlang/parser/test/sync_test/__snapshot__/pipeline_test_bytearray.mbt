fn make(a : Byte, b : Byte, c : Byte) -> FixedArray[Byte] {
  [a, b, c]
}

fn make2(a : Byte, b : Byte, c : Byte) -> FixedArray[Byte] {
  [a, b, c, a, b, c]
}

fn make_array(a : Byte, b : Byte, c : Byte) -> Array[Byte] {
  [a, b, c]
}

fn to_bytes(xs : FixedArray[Byte]) -> Bytes = "%identity"

fn from_bytes(xs : Bytes) -> FixedArray[Byte] = "%identity"

fn main {
  let bs = make(b'\x40', b'\x41', b'\x42')
  println(bs)
  let bs2 = make2(b'\x40', b'\x41', b'\x42')
  println(bs2)
  let b = bs[1]
  println(b)
  bs[2] = b'\x43'
  println(to_bytes(bs))
  let bs1 = FixedArray::make(5, b'\x01')
  println(to_bytes(bs1))
  let bs2 = from_bytes(b"xxx")
  println(bs2)
  let ba = make_array(b'\x40', b'\x41', b'\x42')
  ba.push(b'\x43')
  println(ba)
  let big_bs : FixedArray[Byte] = [
    b'\x01', b'\x02', b'\x03', b'\x04', b'\x05', b'\x06', b'\x07', b'\x08', b'\x09',
    b'\x0a',
  ]
  println(big_bs)
}
