fn init {
  let a = b"moonbit_rabbit"
} 

fn error() -> Unit {
  let _ = b"moonbit🐇rabbit🐰" // error: ASCII in bytes literal is not allowed
  let _ = b"\xGG" // error: invalid hex escape
  let _ = b"\oFFF" // error: invalid oct escape
}

let g = b"unfinished bytes literal

  