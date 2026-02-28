let output_string:(String) -> Unit = fn(n) {
  for c in n {
    println(c)
  }
}

fn init {
  println("hello, ")
}

let s = "hello, world"

let x = 7

fn init {
  let mut i = x
  while i < s.length() {
    println(s.get_char(i).unwrap())
    i = i + 1
  }
}

