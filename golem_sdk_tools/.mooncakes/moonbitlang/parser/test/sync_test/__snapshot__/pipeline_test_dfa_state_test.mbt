

fn parse_digit(s : StringView) -> (Int64?, StringView) {
  lexmatch s with longest {
    ("-?[0-9][0-9_]+[0-9]" as digit, rest) => {
      (Some (try! @strconv.parse_int64(digit)),rest)
    }
    _ => (None,s)
  }
}


fn main {
  println(parse_digit("123 ab"))
  println(parse_digit("-323c"))
  println(parse_digit("123_456"))
  println(parse_digit("123_456_789d"))
  println(parse_digit("123_456_789_0"))
  println(parse_digit("123_456_789_01"))
  println(parse_digit("123_456_789_012"))
  println(parse_digit("123_456_789_0123"))
  println(parse_digit("123_456_789_01234"))
  println(parse_digit("123_456_789_012345"))
  println(parse_digit("123_456_789_0123456"))
  println("failed cases")
  println(parse_digit("_323"))
  println(parse_digit("323_"))
  println(parse_digit("3_23_"))
}
