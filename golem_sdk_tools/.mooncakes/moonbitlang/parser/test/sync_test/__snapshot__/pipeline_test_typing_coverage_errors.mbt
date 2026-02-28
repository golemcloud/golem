suberror ParseError {
  Empty
  InvalidChar(Char)
} derive(Show, Eq)

fn first_char(s : String) -> Char raise ParseError {
  if s is "" {
    raise ParseError::Empty
  }
  match s.get_char(0) {
    Some(c) => c
    None => raise ParseError::Empty
  }
}

fn digit_value(c : Char) -> Int raise ParseError {
  if c.compare('0') < 0 || c.compare('9') > 0 {
    raise ParseError::InvalidChar(c)
  }
  c.to_int() - '0'.to_int()
}

fn parse_two(s : String) -> Int raise ParseError {
  let c0 = first_char(s)
  let c1 = match s.get_char(1) {
    Some(c) => c
    None => raise ParseError::Empty
  }
  digit_value(c0) * 10 + digit_value(c1)
}

fn safe_div(x : Int, y : Int) -> Int raise {
  if y == 0 {
    fail("division by zero")
  }
  x / y
}

fn parse_or_default(s : String, default : Int) -> Int {
  try parse_two(s) catch {
    ParseError::Empty => default
    ParseError::InvalidChar(_) => default
  } noraise {
    v => v
  }
}

fn init {
  let r1 : Result[Int, ParseError] = try? parse_two("42")
  let r2 : Result[Int, ParseError] = try? parse_two("x3")
  let _ = parse_or_default("7", -1)
  let _ = parse_or_default("", -1)
  let _ : Result[Int, Error] = try? safe_div(10, 2)
  let _ : Result[Int, Error] = try? safe_div(1, 0)
  let _ = r1
  let _ = r2
  ()
}
