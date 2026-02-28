///|
fn offset_to_line_column(content : String, offset : Int) -> (Int, Int) {
  let mut line = 1
  let mut column = 1
  for i in 0..<offset {
    if content.code_unit_at(i) == '\n' {
      line += 1
      column = 1
    } else {
      column += 1
    }
  }
  (line, column)
}
