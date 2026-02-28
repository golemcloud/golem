pub fn to_string(selfx : Int) -> String {
  fn abs(n : Int) -> Int {
    if n < 0 {
      0 - n
    } else {
      n
    }
  }

  // The min and max value of i32 are -2147483648 and 2147483647,
  // so max=11 is enough.

  let buf = StringBuilder::new(size_hint=11)
  if selfx < 0 {
    buf.write_char('-')
  }
  fn write_digits(num) {
    let num2 = num / 10
    if num2 != 0 {
      write_digits(num2)
    }
    buf.write_char((abs(num % 10) + 48).unsafe_to_char())
  }

  write_digits(selfx)
  buf.to_string()
}

fn init {
  println(to_string(333))
}
