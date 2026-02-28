
fn decimal_digit_value(c: Char) -> Int {
  c.to_int() - '0'.to_int()
}

fn hexdecimal_digit_value(c: Char) -> Int {
  if '0' <= c && c <= '9' {
    c.to_int() - '0'.to_int()
  } else if 'a' <= c && c <= 'f' {
    c.to_int() - 'a'.to_int() + 10
  } else if 'A' <= c && c <= 'F' {
    c.to_int() - 'A'.to_int() + 10
  } else {
    -1
  }
}

fn toggle_case(c: Char) -> Char {
  if 'a' <= c && c <= 'z' {
    (c.to_int() - 'a'.to_int() + 'A'.to_int()).unsafe_to_char()
  } else if 'A' <= c && c <= 'Z' {
    (c.to_int() - 'A'.to_int() + 'a'.to_int()).unsafe_to_char()
  } else {
    c
  }
}

fn init {
  println(decimal_digit_value('0'));
  println(decimal_digit_value('5'));
  println(decimal_digit_value('9'));
  println(hexdecimal_digit_value('1'));
  println(hexdecimal_digit_value('8'));
  println(hexdecimal_digit_value('b'));
  println(hexdecimal_digit_value('F'));
  println(toggle_case('x'))
  println(toggle_case('W'))
  println(toggle_case('0'))
}
