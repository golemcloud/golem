fn check(b : Bytes, arr : FixedArray[Int]) -> Unit {
  fn f(bool : Bool) -> Unit {
    if bool {
      println("o")
    } else {
      println("x")
    }
  }
  f(b.length()==arr.length())
  println(" ")
  for i = 0; i < arr.length() && i < b.length(); i = i + 1 {
    f(b[i]==arr[i].to_byte())
  }

}

let g = b"ab\xFF\o000\r\ncd"
let g0 = b"\xff"
let g1 : Bytes= "\xff"
let g2 : Bytes= b"\x1f"
let g3 : Bytes = "\x1f"

let g4 : Bytes = "你好"
fn init {
  println("g0 len is \{g0.length()}")  
  println("g1 len is \{g1.length()}")  
  println("g2 len is \{g2.length()}")  
  println("g3 len is \{g3.length()}")  
  println("g4 len is \{g4.length()}")  
  let a = b"abcd"
  let b = b"1234"
  check(a, [97, 98, 99, 100])
  check(b, [49, 50, 51, 52])
  check(g, [97, 98, 255, 0, 13, 10, 99, 100])
}

fn init {
  let b1 = b"\x00\x00\x00\x00".to_fixedarray()
  b1[0] = b'\x01'
  println(b"\x00\x00\x00\x00")
  println(b1)
}
