#cfg(any(target="native", target="llvm"))
#borrow(bytes)
extern "C" fn strlen(bytes : Bytes) -> Int = "strlen"

#cfg(not(any(target="native", target="llvm")))
fn strlen(bytes : Bytes) -> Int {
  for i in 0..<bytes.length() {
    if bytes[i] == 0 {
      break i
    }
  } else {
    bytes.length()
  }
}

fn main {
  println(strlen(b"abcd"))
  println(strlen(b"ab\x00cd"))
  println(strlen(Bytes::make(4, b'a')))
  println(strlen(Bytes::make(4, 0)))
  println(strlen([ 'a', 'a' + 1, 'a' + 2, 'a' + 3 ]))
}
